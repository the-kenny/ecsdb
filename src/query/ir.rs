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

impl Query {
    pub fn into_sql(self) -> (String, Vec<(String, Box<dyn ToSql>)>) {
        let mut select = self.filter.sql_query();
        let order_by = match self.order_by {
            OrderBy::Asc => "order by entity asc",
            OrderBy::Desc => "order by entity desc",
        };

        select.sql = format!("{} {}", select.sql, order_by);

        (select.sql, select.placeholders)
    }
}

#[derive(Debug, PartialEq)]
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
                let exprs: Vec<_> = exprs
                    .into_iter()
                    .filter(|e| *e != None)
                    .map(Self::simplify)
                    .collect();

                Or(exprs)
            }
            And(exprs) => {
                let exprs: Vec<_> = exprs
                    .into_iter()
                    .filter(|e| *e != None)
                    .map(Self::simplify)
                    .collect();
                And(exprs)
            }
            other => other,
        }
    }
}

impl FilterExpression {
    fn sql_query(&self) -> SqlFragment<Select> {
        let filter = self.where_clause();
        let sql = format!(
            "select distinct entity from components where {}",
            filter.sql
        );

        SqlFragment {
            kind: PhantomData,
            sql,
            placeholders: filter.placeholders,
        }
    }

    fn where_clause(&self) -> SqlFragment<Where> {
        match self {
            FilterExpression::None => SqlFragment::new("true", []),

            FilterExpression::WithComponent(c) => SqlFragment::new(
                "entity in (select entity from components where component = ?1)",
                [("?1", Box::new(c.to_owned()) as _)],
            ),

            FilterExpression::WithoutComponent(c) => SqlFragment::new(
                "entity not in (select entity from components where component = ?1)",
                [("?1", Box::new(c.to_owned()) as _)],
            ),

            FilterExpression::EntityId(id) => {
                SqlFragment::new("entity = ?1", [("?1", Box::new(*id) as _)])
            }

            FilterExpression::WithComponentData(component, data) => {
                if matches!(data, rusqlite::types::Value::Null) {
                    SqlFragment::new(
                        "entity in (select entity from components where component = ?1 and data is null)",
                        [("?1", Box::new(component.to_owned()) as _)],
                    )
                } else {
                    SqlFragment::new(
                        "entity in (select entity from components where component = ?1 and data = ?2)",
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
                use rusqlite::types::Value;

                let (range_filter_condition, mut params) = match (start, end) {
                    (Value::Null, Value::Null) => (
                        "data is null",
                        vec![]
                    ),
                    (Value::Null, end) =>  (
                            "velodb_extract_data(data) <= velodb_extract_data(?2)",
                            vec![
                                ("?2", Box::new(end.to_owned()) as _),
                            ],
                        ),
                    (start, Value::Null) =>  (
                            "velodb_extract_data(data) >= velodb_extract_data(?2)",
                            vec![
                                ("?2", Box::new(start.to_owned()) as _),
                            ],
                        ),

                    (start, end) =>  (
                            "velodb_extract_data(data) between velodb_extract_data(?2) and velodb_extract_data(?3)",
                            vec![
                                ("?2", Box::new(start.to_owned()) as _),
                                ("?3", Box::new(end.to_owned()) as _),
                            ],
                        ),
                };

                let sql = format!("entity in (select entity from components where component = ?component and {range_filter_condition})");
                params.push(("?component", Box::new(component.to_owned()) as _));
                SqlFragment::new(&sql, params)
            }

            FilterExpression::And(exprs) => Self::combine_exprs("and", exprs),
            FilterExpression::Or(exprs) => Self::combine_exprs("or", exprs),
        }
    }

    fn combine_exprs(via: &str, exprs: &[FilterExpression]) -> SqlFragment<Where> {
        let mut exprs = exprs.into_iter().map(|e| e.where_clause());

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
            fragment.placeholders.extend(expr.placeholders.into_iter());
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

        for (idx, (a, _)) in mappings.iter().enumerate() {
            self.sql = self.sql.replace(a, &format!(":{idx}:"));
        }

        for (idx, (_, b)) in mappings.iter().enumerate() {
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
                    .map(|(p, _v)| (p, format_args!("<dyn ToSql>")))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod test {
    use insta::assert_debug_snapshot;

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
                FilterExpression::with_component("ecsdb::Foo"),
                FilterExpression::without_component("ecsdb::Bar"),
            ]),
            FilterExpression::or([
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
        ]
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

    #[test]
    pub fn filter_expression_sql_query() {
        for case in cases() {
            let expr = format!("{case:?}.sql_query()");
            insta::with_settings!({omit_expression => true, description => &expr, snapshot_suffix => &expr}, {
                assert_debug_snapshot!(case.sql_query());
            });
        }
    }
}
