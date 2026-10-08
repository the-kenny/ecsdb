use std::{
    collections::{BTreeMap, HashSet},
    marker::PhantomData,
};

use rusqlite::ToSql;

use crate::EntityId;

#[derive(Debug)]
pub enum OrderBy {
    Asc,
    Desc,
}

#[derive(Debug)]
pub struct Query {
    pub filter: FilterExpression,
    pub order_by: OrderBy,
}

pub(crate) type Sql = String;
pub(crate) type SqlParameters = Vec<(String, Box<dyn ToSql>)>;
type StaticPlaceholders = Vec<(&'static str, Box<dyn ToSql>)>;

impl Query {
    pub(crate) fn into_sql(self) -> (Sql, SqlParameters) {
        let mut select = self.filter.simplify().sql_query();
        let order_by = match self.order_by {
            OrderBy::Asc => "order by entity asc",
            OrderBy::Desc => "order by entity desc",
        };

        select.sql = format!("{} {}", select.sql, order_by);

        (select.sql, select.placeholders)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FilterExpression {
    None,

    And(Vec<FilterExpression>),
    Or(Vec<FilterExpression>),

    EntityId(EntityId),
    WithComponent(String),
    WithoutComponent(String),

    WithComponentData(String, rusqlite::types::Value),
    WithComponentDataRange {
        component: String,
        start: rusqlite::types::Value,
        end: rusqlite::types::Value,
    },
}

impl FilterExpression {
    pub fn none() -> Self {
        Self::None
    }

    pub fn with_component(c: &str) -> Self {
        Self::WithComponent(c.to_owned())
    }

    pub fn without_component(c: &str) -> Self {
        Self::WithoutComponent(c.to_owned())
    }

    pub fn with_component_data(c: &str, value: rusqlite::types::Value) -> Self {
        Self::WithComponentData(c.to_owned(), value)
    }

    pub fn entity(e: EntityId) -> Self {
        Self::EntityId(e)
    }

    pub fn and(exprs: impl IntoIterator<Item = FilterExpression>) -> Self {
        Self::And(exprs.into_iter().collect())
    }

    pub fn or(exprs: impl IntoIterator<Item = FilterExpression>) -> Self {
        Self::Or(exprs.into_iter().collect())
    }
}

impl FilterExpression {
    pub fn simplify(self) -> Self {
        use FilterExpression::*;

        match self {
            Or(exprs) => {
                let exprs = exprs
                    .into_iter()
                    .filter(|e| *e != None)
                    // Flatten nested `Or`
                    .flat_map(|e| match e {
                        Or(exprs) => exprs,
                        other => vec![other],
                    })
                    .map(Self::simplify);

                let mut deduplicated = Vec::new();
                for expr in exprs {
                    if !deduplicated.contains(&expr) {
                        deduplicated.push(expr)
                    }
                }

                Or(deduplicated)
            }
            And(exprs) => {
                let exprs = exprs
                    .into_iter()
                    .filter(|e| *e != None)
                    // Flatten nested `And`
                    .flat_map(|e| match e {
                        And(exprs) => exprs,
                        other => vec![other],
                    })
                    .map(Self::simplify);

                let mut deduplicated = Vec::new();
                for expr in exprs {
                    if !deduplicated.contains(&expr) {
                        deduplicated.push(expr)
                    }
                }

                And(deduplicated)
            }
            other => other,
        }
    }
}

/// A filter that can drive the outer scan of a query directly via the
/// covering `(component, entity)` index instead of a correlated subquery.
struct Anchor {
    fragment: SqlFragment<Where>,
    /// `component in (...)` can yield the same entity once per matching
    /// component, so the outer select needs `distinct`. Single-component
    /// anchors never do because `(entity, component)` is unique.
    distinct: bool,
}

impl FilterExpression {
    /// Builds `select entity from components where ...`.
    ///
    /// If the (simplified) expression is a leaf or a top-level `And` that
    /// contains an anchorable filter (`WithComponent`, `WithComponentData`,
    /// `WithComponentDataRange`, or an `Or` consisting solely of
    /// `WithComponent`), that filter is emitted as a plain predicate on the
    /// outer `components` row so SQLite can seek the `(component, entity)`
    /// index. All remaining filters keep their correlated-subquery form.
    /// Without an anchor the query degrades to a full scan over `components`.
    fn sql_query(&self) -> SqlFragment<Select> {
        let candidates: &[FilterExpression] = match self {
            FilterExpression::And(exprs) => exprs,
            other => std::slice::from_ref(other),
        };

        // Most selective first.
        let priorities: [fn(&FilterExpression) -> bool; 4] = [
            |e| matches!(e, FilterExpression::WithComponentData(..)),
            |e| matches!(e, FilterExpression::WithComponentDataRange { .. }),
            |e| matches!(e, FilterExpression::WithComponent(_)),
            |e| e.is_component_in_list(),
        ];

        let anchor_idx = priorities
            .iter()
            .find_map(|is_anchor| candidates.iter().position(is_anchor));

        let Some(anchor_idx) = anchor_idx else {
            let filter = self.where_clause();
            return SqlFragment {
                kind: PhantomData,
                sql: format!(
                    "select distinct entity from components where {}",
                    filter.sql
                ),
                placeholders: filter.placeholders,
            };
        };

        let anchor = candidates[anchor_idx]
            .anchor()
            .expect("anchor predicate matched a non-anchorable expression");

        let remaining = candidates
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != anchor_idx)
            .map(|(_, expr)| expr.where_clause());

        let filter =
            Self::combine_fragments("and", std::iter::once(anchor.fragment).chain(remaining));

        let distinct = if anchor.distinct { "distinct " } else { "" };

        SqlFragment {
            kind: PhantomData,
            sql: format!(
                "select {distinct}entity from components where {}",
                filter.sql
            ),
            placeholders: filter.placeholders,
        }
    }

    /// `Or` whose children are all `WithComponent` → `component in (...)`.
    fn is_component_in_list(&self) -> bool {
        match self {
            FilterExpression::Or(exprs) => {
                !exprs.is_empty()
                    && exprs
                        .iter()
                        .all(|e| matches!(e, FilterExpression::WithComponent(_)))
            }
            _ => false,
        }
    }

    fn anchor(&self) -> Option<Anchor> {
        use rusqlite::types::Value;

        let fragment = match self {
            FilterExpression::WithComponent(c) => {
                SqlFragment::new("component = ?1", [("?1", Box::new(c.to_owned()) as _)])
            }

            FilterExpression::WithComponentData(component, Value::Null) => SqlFragment::new(
                "component = ?1 and data is null",
                [("?1", Box::new(component.to_owned()) as _)],
            ),

            FilterExpression::WithComponentData(component, data) => SqlFragment::new(
                "component = ?1 and data = ?2",
                [
                    ("?1", Box::new(component.to_owned()) as _),
                    ("?2", Box::new(data.to_owned()) as _),
                ],
            ),

            FilterExpression::WithComponentDataRange {
                component,
                start,
                end,
            } => {
                let (range_filter_condition, mut params) =
                    Self::range_condition("data", start, end);
                params.push(("?component", Box::new(component.to_owned()) as _));
                SqlFragment::new(
                    &format!("component = ?component and {range_filter_condition}"),
                    params,
                )
            }

            FilterExpression::Or(exprs) if self.is_component_in_list() => {
                let placeholders: Vec<(String, Box<dyn ToSql>)> = exprs
                    .iter()
                    .enumerate()
                    .map(|(idx, e)| {
                        let FilterExpression::WithComponent(c) = e else {
                            unreachable!("checked by is_component_in_list")
                        };
                        (format!("?{}", idx + 1), Box::new(c.to_owned()) as _)
                    })
                    .collect();

                let list = placeholders
                    .iter()
                    .map(|(p, _)| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");

                return Some(Anchor {
                    fragment: SqlFragment {
                        kind: PhantomData,
                        sql: format!("component in ({list})"),
                        placeholders,
                    },
                    distinct: true,
                });
            }

            _ => return None,
        };

        Some(Anchor {
            fragment,
            distinct: false,
        })
    }

    /// Range predicate on `{column}`; `?2`/`?3` are the bound placeholders.
    fn range_condition(
        column: &str,
        start: &rusqlite::types::Value,
        end: &rusqlite::types::Value,
    ) -> (String, StaticPlaceholders) {
        use rusqlite::types::Value;

        match (start, end) {
            (Value::Null, Value::Null) => (format!("{column} is null"), vec![]),
            (Value::Null, end) => (
                format!("velodb_extract_data({column}) <= velodb_extract_data(?2)"),
                vec![("?2", Box::new(end.to_owned()) as _)],
            ),
            (start, Value::Null) => (
                format!("velodb_extract_data({column}) >= velodb_extract_data(?2)"),
                vec![("?2", Box::new(start.to_owned()) as _)],
            ),
            (start, end) => (
                format!(
                    "velodb_extract_data({column}) between velodb_extract_data(?2) and velodb_extract_data(?3)"
                ),
                vec![
                    ("?2", Box::new(start.to_owned()) as _),
                    ("?3", Box::new(end.to_owned()) as _),
                ],
            ),
        }
    }

    fn where_clause(&self) -> SqlFragment<Where> {
        match self {
            FilterExpression::None => SqlFragment::new("true", []),

            FilterExpression::WithComponent(c) => SqlFragment::new(
                "(select true from components c2 where c2.entity = components.entity and c2.component = ?1)",
                [("?1", Box::new(c.to_owned()) as _)],
            ),

            FilterExpression::WithoutComponent(c) => SqlFragment::new(
                "(select true from components c2 where c2.entity = components.entity and c2.component = ?1) is null",
                [("?1", Box::new(c.to_owned()) as _)],
            ),

            FilterExpression::EntityId(id) => {
                SqlFragment::new("entity = ?1", [("?1", Box::new(*id) as _)])
            }

            FilterExpression::WithComponentData(component, data) => {
                if matches!(data, rusqlite::types::Value::Null) {
                    SqlFragment::new(
                        "(select true from components c2 where c2.entity = components.entity and c2.component = ?1 and c2.data is null)",
                        [("?1", Box::new(component.to_owned()) as _)],
                    )
                } else {
                    SqlFragment::new(
                        "(select true from components c2 where c2.entity = components.entity and c2.component = ?1 and c2.data = ?2)",
                        [
                            ("?1", Box::new(component.to_owned()) as _),
                            ("?2", Box::new(data.to_owned()) as _),
                        ],
                    )
                }
            }

            FilterExpression::WithComponentDataRange {
                component,
                start,
                end,
            } => {
                let (range_filter_condition, mut params) =
                    Self::range_condition("c2.data", start, end);

                let sql = format!(
                    "(select true from components c2 where c2.entity = components.entity and c2.component = ?component and {range_filter_condition})"
                );
                params.push(("?component", Box::new(component.to_owned()) as _));
                SqlFragment::new(&sql, params)
            }
            FilterExpression::And(exprs) => Self::combine_exprs("and", exprs),
            FilterExpression::Or(exprs) => Self::combine_exprs("or", exprs),
        }
    }

    fn combine_exprs(via: &str, exprs: &[FilterExpression]) -> SqlFragment<Where> {
        Self::combine_fragments(via, exprs.iter().map(|e| e.where_clause()))
    }

    /// Joins fragments with `via`, renumbering all placeholders to a single
    /// sequential `:n` namespace.
    fn combine_fragments(
        via: &str,
        exprs: impl IntoIterator<Item = SqlFragment<Where>>,
    ) -> SqlFragment<Where> {
        let mut exprs = exprs.into_iter();

        let Some(fragment) = exprs.next() else {
            return FilterExpression::None.where_clause();
        };

        let mut last_placeholder = 0;

        let mut rename_fn = |_old| {
            last_placeholder += 1;
            let n = last_placeholder;
            format!(":{n}")
        };

        let mut fragment = fragment.rename_identifier(&mut rename_fn);

        for expr in exprs {
            let expr = expr.rename_identifier(&mut rename_fn);
            fragment.sql = format!("{} {via} {}", fragment.sql, expr.sql);
            fragment.placeholders.extend(expr.placeholders);
        }

        fragment.sql = format!("({})", fragment.sql);

        assert_eq!(
            fragment.placeholders.len(),
            fragment
                .placeholders
                .iter()
                .map(|(p, _)| p)
                .collect::<HashSet<_>>()
                .len()
        );

        fragment
    }
}

#[derive(Debug)]
struct Where;
#[derive(Debug)]
struct Select;

struct SqlFragment<T> {
    pub kind: PhantomData<T>,
    pub sql: String,
    pub placeholders: Vec<(String, Box<dyn ToSql>)>,
}

impl<T> SqlFragment<T> {
    pub fn new<'a>(
        sql: &str,
        placeholders: impl IntoIterator<Item = (&'a str, Box<dyn ToSql>)>,
    ) -> Self {
        Self {
            kind: PhantomData,
            sql: sql.to_owned(),
            placeholders: placeholders
                .into_iter()
                .map(|(p, v)| (p.to_string(), v))
                .collect(),
        }
    }

    pub fn rename_identifier(mut self, mut fun: impl FnMut(String) -> String) -> Self {
        let mappings: BTreeMap<_, _> = self
            .placeholders
            .iter()
            .map(|(p, _)| (p.to_owned(), fun(p.to_owned())))
            .collect();

        // Replace longer names first so `?1` doesn't clobber `?10`.
        let mut ordered: Vec<(&String, &String)> = mappings.iter().collect();
        ordered.sort_by(|(a, _), (b, _)| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));

        for (idx, (a, _)) in ordered.iter().enumerate() {
            self.sql = self.sql.replace(a.as_str(), &format!(":{idx}:"));
        }

        for (idx, (_, b)) in ordered.iter().enumerate() {
            self.sql = self.sql.replace(&format!(":{idx}:"), b);
        }

        for (placeholder, _value) in self.placeholders.iter_mut() {
            *placeholder = mappings[placeholder].clone();
        }

        self
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for SqlFragment<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(&format!("SqlFragment<{}>", std::any::type_name::<T>()))
            .field("sql", &self.sql)
            .field(
                "placeholders",
                &self
                    .placeholders
                    .iter()
                    .map(|(p, v)| {
                        use rusqlite::types::{ToSqlOutput, Value};
                        let v: Value = match v.to_sql().unwrap() {
                            ToSqlOutput::Borrowed(v) => Value::from(v),
                            ToSqlOutput::Owned(v) => v,
                            other => unreachable!("Unexpected ToSqlOutput {other:?}"),
                        };

                        (p, v)
                    })
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod test {
    use insta::assert_debug_snapshot;
    use rusqlite::types::Value;

    use crate::query::ir::FilterExpression;

    fn cases() -> Vec<FilterExpression> {
        vec![
            FilterExpression::none(),
            FilterExpression::with_component("ecsdb::Test"),
            FilterExpression::without_component("ecsdb::Test"),
            FilterExpression::entity(42),
            FilterExpression::and([
                FilterExpression::with_component("ecsdb::Test"),
                FilterExpression::entity(42),
            ]),
            FilterExpression::and([
                FilterExpression::with_component("ecsdb::Test"),
                FilterExpression::entity(42),
                FilterExpression::with_component("ecsdb::Test"),
                FilterExpression::entity(42),
            ]),
            FilterExpression::and([
                FilterExpression::with_component("ecsdb::Foo"),
                FilterExpression::without_component("ecsdb::Bar"),
            ]),
            FilterExpression::or([
                FilterExpression::with_component("ecsdb::Test"),
                FilterExpression::entity(42),
            ]),
            FilterExpression::or([
                FilterExpression::with_component("ecsdb::Test"),
                FilterExpression::entity(42),
                FilterExpression::with_component("ecsdb::Test"),
                FilterExpression::entity(42),
            ]),
            FilterExpression::or([
                FilterExpression::with_component("ecsdb::Foo"),
                FilterExpression::without_component("ecsdb::Bar"),
            ]),
            FilterExpression::or([
                FilterExpression::and([
                    FilterExpression::entity(42),
                    FilterExpression::with_component("ecsdb::Test"),
                ]),
                FilterExpression::and([
                    FilterExpression::entity(23),
                    FilterExpression::with_component("ecsdb::Foo"),
                    FilterExpression::without_component("ecsdb::Bar"),
                ]),
            ]),
            FilterExpression::and([
                FilterExpression::and([
                    FilterExpression::entity(42),
                    FilterExpression::with_component("ecsdb::Test"),
                ]),
                FilterExpression::and([
                    FilterExpression::entity(23),
                    FilterExpression::with_component("ecsdb::Foo"),
                    FilterExpression::without_component("ecsdb::Bar"),
                ]),
            ]),
            FilterExpression::or([
                FilterExpression::or([
                    FilterExpression::entity(42),
                    FilterExpression::with_component("ecsdb::Test"),
                ]),
                FilterExpression::and([
                    FilterExpression::entity(23),
                    FilterExpression::with_component("ecsdb::Foo"),
                    FilterExpression::without_component("ecsdb::Bar"),
                ]),
            ]),
            // Anchoring cases
            FilterExpression::with_component_data("ecsdb::Test", Value::Text("x".into())),
            FilterExpression::with_component_data("ecsdb::Test", Value::Null),
            FilterExpression::WithComponentDataRange {
                component: "ecsdb::Test".into(),
                start: Value::Integer(1),
                end: Value::Integer(9),
            },
            FilterExpression::and([
                FilterExpression::with_component("ecsdb::A"),
                FilterExpression::with_component("ecsdb::B"),
                FilterExpression::without_component("ecsdb::C"),
            ]),
            FilterExpression::and([
                FilterExpression::without_component("ecsdb::C"),
                FilterExpression::with_component_data("ecsdb::B", Value::Integer(7)),
            ]),
            FilterExpression::and([FilterExpression::without_component("ecsdb::C")]),
            FilterExpression::or([
                FilterExpression::with_component("ecsdb::A"),
                FilterExpression::with_component("ecsdb::B"),
            ]),
            FilterExpression::and([
                FilterExpression::or([
                    FilterExpression::with_component("ecsdb::A"),
                    FilterExpression::with_component("ecsdb::B"),
                ]),
                FilterExpression::without_component("ecsdb::C"),
            ]),
            FilterExpression::and([
                FilterExpression::with_component("ecsdb::A"),
                FilterExpression::or([
                    FilterExpression::with_component("ecsdb::B"),
                    FilterExpression::with_component("ecsdb::C"),
                ]),
            ]),
        ]
    }

    #[test]
    fn simplify() {
        for case in cases() {
            let expr = format!("{case:?}.simplify()");
            insta::with_settings!({omit_expression => true, description => &expr, snapshot_suffix => &expr}, {
                assert_debug_snapshot!(case.simplify());
            });
        }
    }

    #[test]
    pub fn filter_expression_where_clause() {
        for case in cases() {
            let expr = format!("{case:?}.where_clause()");
            insta::with_settings!({omit_expression => true, description => &expr, snapshot_suffix => &expr}, {
                assert_debug_snapshot!(case.where_clause());
            });
        }
    }

    /// `?1` must not clobber `?10` when renumbering placeholders.
    #[test]
    fn in_list_with_ten_or_more_placeholders() {
        let names: Vec<String> = (0..12).map(|i| format!("ecsdb::C{i}")).collect();
        let expr = FilterExpression::or(names.iter().map(|n| FilterExpression::with_component(n)));

        let fragment = expr.sql_query();
        assert_eq!(fragment.placeholders.len(), 12);

        for (idx, (placeholder, _)) in fragment.placeholders.iter().enumerate() {
            assert_eq!(placeholder, &format!(":{}", idx + 1));
        }

        let expected = (1..=12)
            .map(|n| format!(":{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(
            fragment.sql,
            format!("select distinct entity from components where (component in ({expected}))")
        );
    }

    #[test]
    pub fn filter_expression_sql_query() {
        for case in cases() {
            let expr = format!("{case:?}.sql_query()");
            insta::with_settings!({omit_expression => true, description => &expr, snapshot_suffix => &expr}, {
                assert_debug_snapshot!(case.sql_query());
            });

            let expr = format!("{case:?}.simplify().sql_query()");
            insta::with_settings!({omit_expression => true, description => &expr, snapshot_suffix => &expr}, {
                assert_debug_snapshot!(case.simplify().sql_query());
            });
        }
    }
}
