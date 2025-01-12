use crate::EntityId;

use super::{sql::Components, Component};
use std::marker::PhantomData;

pub trait Filter {
    fn sql_query() -> sea_query::SelectStatement;
}

pub trait DataFilter: Sized {
    fn sql_query(self) -> sea_query::SelectStatement;
}

impl DataFilter for () {
    fn sql_query(self) -> sea_query::SelectStatement {
        <Self as Filter>::sql_query()
    }
}

impl DataFilter for EntityId {
    fn sql_query(self) -> sea_query::SelectStatement {
        use sea_query::*;
        Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .and_where(Expr::col(Components::Entity).eq(self))
            .limit(1)
            .take()
    }
}

impl<C> DataFilter for C
where
    C: Component,
{
    fn sql_query(self) -> sea_query::SelectStatement {
        use sea_query::*;
        let expr = match C::to_rusqlite(self).unwrap() {
            rusqlite::types::Value::Blob(blob) => {
                struct Unhex;
                impl Iden for Unhex {
                    fn unquoted(&self, s: &mut dyn Write) {
                        write!(s, "unhex").unwrap();
                    }
                }

                Expr::col(Components::Data).eq(Func::cust(Unhex).arg(hex::encode_upper(blob)))
            }
            rusqlite::types::Value::Text(json) => Expr::col(Components::Data).eq(json),
            rusqlite::types::Value::Null => Expr::col(Components::Data).is_null(),
            other => unreachable!("{other:?}"),
        };
        <Self as Filter>::sql_query().and_where(expr).take()
    }
}

pub struct Query<'a, F, D = ()>
where
    F: ?Sized,
{
    pub(crate) ecs: &'a crate::Ecs,
    pub(crate) component_filter: PhantomData<F>,
    pub(crate) data_filter: D,
}

impl<'a, F, D> Query<'a, F, D> {
    pub fn new(ecs: &'a crate::Ecs, data_filter: D) -> Self {
        Self {
            ecs,
            component_filter: PhantomData::default(),
            data_filter,
        }
    }
}

impl<'a, F, D> Query<'a, F, D>
where
    F: Filter,
    D: DataFilter + Copy,
{
    pub(crate) fn as_sql_query(&self) -> sea_query::SelectStatement {
        let Query { data_filter, .. } = self;
        and(<F as Filter>::sql_query(), data_filter.sql_query())
            .distinct()
            .take()
    }

    pub fn try_iter(&self) -> Result<impl Iterator<Item = crate::Entity<'a>> + 'a, crate::Error> {
        self.ecs.fetch(self.as_sql_query())
    }

    pub fn iter(&self) -> impl Iterator<Item = crate::Entity<'a>> + 'a {
        self.try_iter().unwrap()
    }
}

impl<'a, F, D> Query<'a, F, D>
where
    F: Filter,
    D: DataFilter,
{
    pub(crate) fn into_sql_query(self) -> sea_query::SelectStatement {
        let Query { data_filter, .. } = self;
        and(<F as Filter>::sql_query(), data_filter.sql_query())
            .distinct()
            .take()
    }

    pub fn db(&self) -> &crate::Ecs {
        &self.ecs
    }

    pub fn try_into_iter(
        self,
    ) -> Result<impl Iterator<Item = crate::Entity<'a>> + 'a, crate::Error> {
        self.ecs.fetch(self.into_sql_query())
    }

    pub fn into_iter(self) -> impl Iterator<Item = crate::Entity<'a>> + 'a {
        self.try_into_iter().unwrap()
    }
}

impl Filter for () {
    fn sql_query() -> sea_query::SelectStatement {
        use sea_query::*;
        Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .take()
    }
}

impl<C: Component> Filter for C {
    fn sql_query() -> sea_query::SelectStatement {
        use sea_query::*;
        sea_query::Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .and_where(Expr::col(Components::Component).eq(C::component_name()))
            .take()
    }
}

pub struct Without<C>(PhantomData<C>);
impl<C: Component> Filter for Without<C> {
    fn sql_query() -> sea_query::SelectStatement {
        use sea_query::*;
        Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .and_where(Expr::col(Components::Entity).not_in_subquery(<C as Filter>::sql_query()))
            .take()
    }
}

pub struct Or<T>(PhantomData<T>);

macro_rules! filter_tuple_impl {
    ($t:tt, $($ts:tt),+) => {
        impl<$t, $($ts,)+> Filter for ($t, $($ts,)+)
        where
            $t: Filter,
            $($ts: Filter,)+
        {
            fn sql_query() -> sea_query::SelectStatement {
                and($t::sql_query(), <($($ts,)+)>::sql_query())
            }
        }

        impl<$t, $($ts,)+> Filter for Or<($t, $($ts,)+)>
        where
            $t: Filter,
            $($ts: Filter,)+
        {
            fn sql_query() -> sea_query::SelectStatement {
                or($t::sql_query(), <($($ts,)+)>::sql_query())
            }
        }

        impl<$t, $($ts,)+> Filter for Without<($t, $($ts,)+)>
        where
            $t: Component,
            $($ts: Component,)+
        {
            fn sql_query() -> sea_query::SelectStatement {
                and(Without::<$t>::sql_query(), Without::<($($ts,)+)>::sql_query())
            }
        }

        impl<$t, $($ts,)+> DataFilter for ($t, $($ts,)+)
        where
            $t: DataFilter,
            $($ts: DataFilter,)+
        {
            fn sql_query(self) -> sea_query::SelectStatement {
                and(self.0.sql_query(), self.1.sql_query())
            }
        }


        filter_tuple_impl!($($ts),+);
    };
    ($t:tt) => {
        impl<$t: Filter> Filter for ($t,) {
            fn sql_query() -> sea_query::SelectStatement {
                $t::sql_query().take()
            }
        }

        impl<$t: Filter> Filter for Or<($t,)> {
            fn sql_query() -> sea_query::SelectStatement {
                $t::sql_query().take()
            }
        }

        impl<$t: Component> Filter for Without<($t,)> {
            fn sql_query() -> sea_query::SelectStatement {
                Without::<$t>::sql_query().take()
            }
        }

        impl<$t: DataFilter> DataFilter for ($t,) {
            fn sql_query(self) -> sea_query::SelectStatement {
                self.0.sql_query()
            }
        }
    };
}

filter_tuple_impl!(A, B, C, D, E, F, G, H, I, J, K, L, M, O, P, Q);
// filter_tuple_impl!(A, B, C);

fn and(a: sea_query::SelectStatement, b: sea_query::SelectStatement) -> sea_query::SelectStatement {
    use sea_query::*;
    Query::select()
        .column(Asterisk)
        .from_subquery(a, NullAlias)
        .union(
            UnionType::Intersect,
            Query::select()
                .column(Asterisk)
                .from_subquery(b, NullAlias)
                .take(),
        )
        .take()
}

fn or(a: sea_query::SelectStatement, b: sea_query::SelectStatement) -> sea_query::SelectStatement {
    use sea_query::*;
    Query::select()
        .column(Asterisk)
        .from_subquery(a, NullAlias)
        .union(
            UnionType::Distinct,
            Query::select()
                .column(Asterisk)
                .from_subquery(b, NullAlias)
                .take(),
        )
        .take()
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate as ecsdb;

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct A;

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct B;

    #[test]
    #[allow(unused)]
    fn system_fns() {
        fn sys_a(query: Query<A>) {}
        fn sys_b(query: Query<(A, Without<B>)>) {}
        fn sys_c(query: Query<Or<(A, B)>>) {}
    }
}
