use ecsdb::{Ecs, Entity};
use nom::bytes::complete::take_while;
use tracing::warn;

#[derive(Debug)]
pub struct Pipeline {
    pub stages: Vec<Stage>,
}

impl Pipeline {
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        use nom::{
            IResult, Parser,
            branch::alt,
            bytes::complete::{tag, tag_no_case},
            character::complete::{char, digit1, multispace0, satisfy},
            combinator::{map, opt, recognize},
            multi::{many0, separated_list1},
            sequence::{delimited, pair, preceded},
        };

        fn ws<'a, O>(
            inner: impl Parser<&'a str, Output = O, Error = nom::error::Error<&'a str>>,
        ) -> impl Parser<&'a str, Output = O, Error = nom::error::Error<&'a str>> {
            delimited(multispace0, inner, multispace0)
        }

        fn single_quoted_string(input: &str) -> IResult<&str, &str> {
            let (input, _) = char('\'').parse(input)?;
            let (input, s) = take_while(|c| c != '\'').parse(input)?;
            let (input, _) = char('\'').parse(input)?;
            Ok((input, s))
        }

        fn json_literal(input: &str) -> IResult<&str, FilterValue> {
            // serde_json's streaming deserializer rejects non-self-
            // delineated values (numbers, null, true, false) that are
            // followed by anything other than JSON whitespace or a JSON
            // delimiter. Our grammar places `)` right after a value in
            // e.g. `filter(entity == -1)`, so a direct parse fails.
            //
            // However, serde_json updates byte_offset to the position
            // right after the parsed value *before* the trailing-char
            // check runs (see serde_json `de.rs` StreamDeserializer::
            // next), so even on the error path the offset tells us
            // where the value ended. Re-parsing that exact prefix
            // succeeds cleanly.
            let mut stream =
                serde_json::Deserializer::from_str(input).into_iter::<serde_json::Value>();
            let result = stream.next();
            let offset = stream.byte_offset();
            let nom_err =
                || nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Alt));
            match result {
                Some(Ok(value)) => Ok((&input[offset..], FilterValue(value))),
                Some(Err(_)) if offset > 0 => {
                    match serde_json::from_str::<serde_json::Value>(&input[..offset]) {
                        Ok(value) => Ok((&input[offset..], FilterValue(value))),
                        Err(_) => Err(nom_err()),
                    }
                }
                _ => Err(nom_err()),
            }
        }

        fn value_literal(input: &str) -> IResult<&str, FilterValue> {
            alt((
                json_literal,
                map(single_quoted_string, |s: &str| {
                    FilterValue(serde_json::Value::String(s.to_owned()))
                }),
            ))
            .parse(input)
        }

        fn cmp_op(input: &str) -> IResult<&str, FilterOperator> {
            alt((
                map(tag("=="), |_| FilterOperator::Eq),
                map(tag("="), |_| FilterOperator::Eq),
                map(tag("!="), |_| FilterOperator::Ne),
                map(tag("<="), |_| FilterOperator::Le),
                map(tag(">="), |_| FilterOperator::Ge),
                map(tag("<"), |_| FilterOperator::Lt),
                map(tag(">"), |_| FilterOperator::Gt),
            ))
            .parse(input)
        }

        fn ident(input: &str) -> IResult<&str, &str> {
            recognize(pair(
                satisfy(|c: char| c.is_alphabetic() || c == '_'),
                take_while(|c: char| c.is_alphanumeric() || c == '_'),
            ))
            .parse(input)
        }

        fn path_segment(input: &str) -> IResult<&str, PathSegment> {
            alt((
                map(preceded(char('.'), ident), |s: &str| {
                    PathSegment::Field(s.to_owned())
                }),
                delimited(
                    char('['),
                    map(digit1, |n: &str| PathSegment::Index(n.parse().unwrap())),
                    char(']'),
                ),
            ))
            .parse(input)
        }

        fn filter_column(input: &str) -> IResult<&str, FilterColumn> {
            alt((
                map(tag("entity"), |_| FilterColumn::Entity),
                map(tag("component"), |_| FilterColumn::Component),
                map(
                    preceded(tag("data"), many0(path_segment)),
                    FilterColumn::Data,
                ),
            ))
            .parse(input)
        }

        fn comparison(input: &str) -> IResult<&str, FilterExpression> {
            let (input, col) = ws(filter_column).parse(input)?;
            let (input, op) = ws(cmp_op).parse(input)?;
            let (input, val) = ws(value_literal).parse(input)?;
            Ok((
                input,
                FilterExpression::Eq {
                    column: col,
                    op,
                    value: val,
                },
            ))
        }

        fn filter_atom(input: &str) -> IResult<&str, FilterExpression> {
            let (input, _) = multispace0.parse(input)?;
            alt((
                delimited(ws(char('(')), filter_or, ws(char(')'))),
                comparison,
            ))
            .parse(input)
        }

        fn filter_and(input: &str) -> IResult<&str, FilterExpression> {
            let (input, first) = filter_atom(input)?;
            let (input, rest) =
                nom::multi::many0(preceded(ws(tag("&&")), filter_atom)).parse(input)?;
            Ok((
                input,
                rest.into_iter().fold(first, |acc, next| {
                    FilterExpression::And(Box::new(acc), Box::new(next))
                }),
            ))
        }

        fn filter_or(input: &str) -> IResult<&str, FilterExpression> {
            let (input, first) = filter_and(input)?;
            let (input, rest) =
                nom::multi::many0(preceded(ws(tag("||")), filter_and)).parse(input)?;
            Ok((
                input,
                rest.into_iter().fold(first, |acc, next| {
                    FilterExpression::Or(Box::new(acc), Box::new(next))
                }),
            ))
        }

        fn filter_stage(input: &str) -> IResult<&str, Stage> {
            let (input, _) = ws(tag("filter")).parse(input)?;
            let (input, expr) = delimited(char('('), ws(filter_or), char(')')).parse(input)?;
            Ok((input, Stage::Filter(expr)))
        }

        fn sort_stage(input: &str) -> IResult<&str, Stage> {
            let (input, _) = ws(tag_no_case("sortBy")).parse(input)?;
            let (input, _) = char('(').parse(input)?;
            let (input, _) = multispace0.parse(input)?;
            let (input, field) = alt((
                map(tag_no_case("created_at"), |_| SortField::CreatedAt),
                map(tag_no_case("last_modified"), |_| SortField::LastModified),
                map(filter_column, SortField::Column),
            ))
            .parse(input)?;
            let (input, _) = multispace0.parse(input)?;
            let (input, order) = opt(alt((
                map(tag_no_case("asc"), |_| SortOrder::Asc),
                map(tag_no_case("desc"), |_| SortOrder::Desc),
            )))
            .parse(input)?;
            let (input, _) = multispace0.parse(input)?;
            let (input, _) = char(')').parse(input)?;
            Ok((input, Stage::SortBy(field, order.unwrap_or(SortOrder::Asc))))
        }

        fn take_stage(input: &str) -> IResult<&str, Stage> {
            let (input, _) = ws(tag("take")).parse(input)?;
            let (input, n) = delimited(char('('), ws(digit1), char(')')).parse(input)?;
            let n: usize = n.parse().unwrap();
            Ok((input, Stage::Take(n)))
        }

        fn skip_stage(input: &str) -> IResult<&str, Stage> {
            let (input, _) = ws(tag("skip")).parse(input)?;
            let (input, n) = delimited(char('('), ws(digit1), char(')')).parse(input)?;
            let n: usize = n.parse().unwrap();
            Ok((input, Stage::Skip(n)))
        }

        fn stage(input: &str) -> IResult<&str, Stage> {
            alt((
                map(ws(tag("all")), |_| Stage::All),
                filter_stage,
                sort_stage,
                take_stage,
                skip_stage,
            ))
            .parse(input)
        }

        fn parse_pipeline(input: &str) -> IResult<&str, Pipeline> {
            let (input, stages) = separated_list1(ws(char('|')), stage).parse(input)?;
            let (input, _) = multispace0.parse(input)?;
            Ok((input, Pipeline { stages }))
        }

        let (remaining, pipe) = parse_pipeline(input).map_err(|e| ParseError(format!("{e}")))?;

        if !remaining.trim().is_empty() {
            return Err(ParseError(format!(
                "Unexpected trailing input: '{remaining}'"
            )));
        }

        Ok(pipe)
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Parse Error: {0}")]
pub struct ParseError(pub String);

#[derive(Debug)]
pub enum Stage {
    All,
    Filter(FilterExpression),
    SortBy(SortField, SortOrder),
    Take(usize),
    Skip(usize),
}

impl Stage {
    pub fn apply<'a>(
        self,
        db: &'a Ecs,
        chain: impl Iterator<Item = Row<'a>> + 'a,
    ) -> impl Iterator<Item = Row<'a>> + 'a {
        match self {
            Stage::All => {
                let rows = db.query::<Entity, ()>().flat_map(|entity| {
                    entity
                        .component_names()
                        .map(move |component| Row { entity, component })
                });
                Box::new(rows) as Box<dyn Iterator<Item = _>>
            }
            Stage::Filter(filter) => Box::new(filter.apply(chain)) as Box<dyn Iterator<Item = _>>,
            Stage::SortBy(field, order) => {
                let mut intermediate: Vec<_> = chain.collect();
                intermediate.sort_unstable_by(|a, b| {
                    let a = (
                        field.extract(a).0.to_string(),
                        a.entity.id(),
                        a.component.as_str(),
                    );
                    let b = (
                        field.extract(b).0.to_string(),
                        b.entity.id(),
                        b.component.as_str(),
                    );
                    let result = a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
                    match order {
                        SortOrder::Asc => result,
                        SortOrder::Desc => result.reverse(),
                    }
                });
                Box::new(intermediate.into_iter())
            }
            Stage::Take(n) => Box::new(chain.take(n)),
            Stage::Skip(n) => Box::new(chain.skip(n)),
        }
    }
}

#[derive(Debug)]
pub enum SortField {
    Column(FilterColumn),
    CreatedAt,
    LastModified,
}

impl SortField {
    fn extract(&self, row: &Row<'_>) -> FilterValue {
        match self {
            Self::Column(column) => column.extract(row),
            Self::CreatedAt => FilterValue(serde_json::Value::String(
                row.entity.created_at().to_rfc3339(),
            )),
            Self::LastModified => FilterValue(serde_json::Value::String(
                row.entity.last_modified().to_rfc2822(),
            )),
        }
    }
}

#[derive(Debug)]
pub enum SortOrder {
    Asc,
    Desc,
}

#[derive(Debug, PartialEq)]
pub struct FilterValue(serde_json::Value);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterColumn {
    Entity,
    Component,
    Data(Vec<PathSegment>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    Field(String),
    Index(usize),
}

fn navigate_path<'a>(
    value: &'a serde_json::Value,
    path: &[PathSegment],
) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path {
        current = match (segment, current) {
            (PathSegment::Field(name), serde_json::Value::Object(m)) => m.get(name)?,
            (PathSegment::Index(i), serde_json::Value::Array(a)) => a.get(*i)?,
            _ => return None,
        };
    }
    Some(current)
}

impl FilterColumn {
    #[tracing::instrument(level = "debug")]
    fn extract(&self, row: &Row) -> FilterValue {
        match self {
            FilterColumn::Entity => FilterValue(serde_json::Value::Number(row.entity.id().into())),
            FilterColumn::Component => {
                FilterValue(serde_json::Value::String(row.component.clone()))
            }
            FilterColumn::Data(path) => {
                let Some(c) = row.entity.dyn_component(&row.component) else {
                    warn!("Failed to get DynComponent");
                    return FilterValue(serde_json::Value::Null);
                };

                let value = match c.kind() {
                    ecsdb::dyn_component::Kind::Json => c.as_json().unwrap(),
                    ecsdb::dyn_component::Kind::Blob => {
                        warn!("'data' filter on BLOB is unimplemented. Row will be skipped");
                        return FilterValue(serde_json::Value::Null);
                    }
                    ecsdb::dyn_component::Kind::Null => {
                        return FilterValue(serde_json::Value::Null);
                    }
                    ecsdb::dyn_component::Kind::Other(r#type) => {
                        unimplemented!("Filter on {:?}", r#type)
                    }
                };

                let resolved = navigate_path(&value, path)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                FilterValue(resolved)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOperator {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug)]
pub enum FilterExpression {
    Eq {
        column: FilterColumn,
        op: FilterOperator,
        value: FilterValue,
    },
    And(Box<FilterExpression>, Box<FilterExpression>),
    Or(Box<FilterExpression>, Box<FilterExpression>),
}

impl FilterExpression {
    pub fn matches(&self, row: &Row<'_>) -> bool {
        match self {
            Self::Eq {
                column,
                op: FilterOperator::Eq,
                value,
            } => column.extract(row) == *value,
            Self::Eq {
                column,
                op: FilterOperator::Ne,
                value,
            } => column.extract(row) != *value,
            Self::Eq { .. } => todo!("{self:?}"),
            Self::And(a, b) => a.matches(row) && b.matches(row),
            Self::Or(a, b) => a.matches(row) || b.matches(row),
        }
    }

    pub fn apply<'a, C>(self, chain: C) -> impl Iterator<Item = C::Item>
    where
        C: Iterator<Item = Row<'a>>,
    {
        chain.filter(move |row| self.matches(row))
    }
}

/// A row in the pipeline — always a full entity with all its components.
#[derive(Debug)]
pub struct Row<'a> {
    entity: ecsdb::Entity<'a>,
    component: String,
}

impl<'a> Row<'a> {
    pub fn entity(&self) -> ecsdb::Entity<'_> {
        self.entity
    }

    pub fn component_name(&self) -> &str {
        &self.component
    }

    pub fn component_value(&self) -> impl std::fmt::Display {
        self.entity
            .dyn_component(&self.component)
            .and_then(|c| c.as_json())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod test {
    use insta::assert_debug_snapshot;

    use super::*;

    const PIPELINES: &[&str] = &[
        "all",
        "all|filter(entity == 42)",
        "all|filter(entity = 42)",
        "all|filter(entity != 42)",
        "all | filter(entity == 42) | filter(entity == 23)",
        "all | take(23)",
        "all | skip(1)",
        "all | skip(1) | sortBy(last_modified)",
        "all|filter(data == null)",
        r#"all|filter(data == [1,2,3])"#,
        r#"all|filter(data == [])"#,
        r#"all|filter(data == {})"#,
        r#"all|filter(data == {"key":"value"})"#,
        r#"all|filter(data == [{"a":1},{"a":2}])"#,
        "all|filter(entity == -1)",
        r#"all|filter(data == "hello\nworld")"#,
        "all|filter(data == 1.5e2)",
        "all|filter(data == 1.5)",
        r#"all|filter(data.last_updated == "foo")"#,
        "all|filter(data.array[3] == 42)",
        r#"all|filter(data.items[0].id == "x")"#,
        "all|filter(data.a.b.c == null)",
        "all|filter(data[0] == 1)",
        "all|sortBy(data.priority)",
        "all|sortBy(data.score desc)",
    ];

    #[test]
    fn query_filters() {
        for &pipeline in PIPELINES {
            insta::with_settings!({ snapshot_suffix => pipeline, description => format!("Pipeline::parse(\"{pipeline}\")") }, {
                assert_debug_snapshot!(Pipeline::parse(pipeline))
            });
        }
    }
}
