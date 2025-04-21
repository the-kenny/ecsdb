use crate::{component::Bundle, Entity, EntityId};

use super::{sql::Components, Component};
use std::{any, marker::PhantomData};

pub trait QueryFilter {
    fn sql_query() -> sea_query::SelectStatement;
}

// pub trait QueryFilterValue {
//     fn sql_query(&self) -> sea_query::SelectStatement;
// }

// pub trait DataFilter: Sized {
//     fn sql_query(self) -> sea_query::SelectStatement;
// }

// impl DataFilter for () {
//     fn sql_query(self) -> sea_query::SelectStatement {
//         <Self as Filter>::sql_query()
//     }
// }

// impl DataFilter for EntityId {
//     fn sql_query(self) -> sea_query::SelectStatement {
//         use sea_query::*;
//         Query::select()
//             .column(Components::Entity)
//             .from(Components::Table)
//             .and_where(Expr::col(Components::Entity).eq(self))
//             .limit(1)
//             .take()
//     }
// }

// impl<C> DataFilter for C
// where
//     C: Component,
// {
//     fn sql_query(self) -> sea_query::SelectStatement {
//         use sea_query::*;
//         let expr = match C::to_rusqlite(self).unwrap() {
//             rusqlite::types::Value::Blob(blob) => {
//                 struct Unhex;
//                 impl Iden for Unhex {
//                     fn unquoted(&self, s: &mut dyn Write) {
//                         write!(s, "unhex").unwrap();
//                     }
//                 }

//                 Expr::col(Components::Data).eq(Func::cust(Unhex).arg(hex::encode_upper(blob)))
//             }
//             rusqlite::types::Value::Text(json) => Expr::col(Components::Data).eq(json),
//             rusqlite::types::Value::Null => Expr::col(Components::Data).is_null(),
//             other => unreachable!("{other:?}"),
//         };
//         <Self as Filter>::sql_query().and_where(expr).take()
//     }
// }

pub struct Query<'a, D = Entity<'a>, F = ()>
where
    F: ?Sized,
{
    pub(crate) ecs: &'a crate::Ecs,
    pub(crate) data: PhantomData<D>,
    pub(crate) filter: PhantomData<F>,
}

impl<'a, C, F> Query<'a, C, F> {
    pub fn new(ecs: &'a crate::Ecs) -> Self {
        Self {
            ecs,
            data: PhantomData,
            filter: PhantomData,
        }
    }
}

impl<'a, D, F> Query<'a, D, F>
where
    D: QueryData + 'a,
    F: QueryFilter,
{
    fn as_sql_query(&self) -> sea_query::SelectStatement {
        intersection(
            <D as QueryData>::sql_query(),
            <F as QueryFilter>::sql_query(),
        )
        .distinct()
        .take()
    }

    // pub fn data_iter<C: QueryData + 'a>(&'a self) -> impl Iterator<Item = C::Output<'a>> + 'a {
    //     self.iter().flat_map(|e| C::from_entity(e))
    // }

    pub fn try_iter(&self) -> Result<impl Iterator<Item = D::Output<'a>> + 'a, crate::Error> {
        let mut query = self.as_sql_query();
        query.order_by(Components::Entity, sea_query::Order::Asc);
        self.ecs.fetch::<D>(query)
    }

    pub fn iter(&self) -> impl Iterator<Item = D::Output<'a>> + 'a {
        self.try_iter().unwrap()
    }

    pub fn try_reverse_iter(
        &self,
    ) -> Result<impl Iterator<Item = D::Output<'a>> + 'a, crate::Error> {
        let mut query = self.as_sql_query();
        query.order_by(Components::Entity, sea_query::Order::Desc);
        self.ecs.fetch::<D>(query)
    }

    pub fn reverse_iter(&self) -> impl Iterator<Item = D::Output<'a>> + 'a {
        self.try_reverse_iter().unwrap()
    }
}

// impl<'a, D, F> Query<'a, D, F>
// where
//     D: QueryData,
//     F: QueryFilter,
// {
//     pub(crate) fn into_sql_query(self) -> sea_query::SelectStatement {
//         let Query { data_filter, .. } = self;

//         // Short circuit to skip `select * from components intersect <real
//         // filter>` type of queries
//         let filter_allows_all = any::type_name::<F>() == any::type_name::<()>();
//         let data_filter_allows_all = any::type_name::<D>() == any::type_name::<()>();

//         let mut query = match (filter_allows_all, data_filter_allows_all) {
//             (true, false) => data_filter.sql_query(),
//             (false, true) => <F as QueryFilter>::sql_query(),
//             _ => intersection(<F as QueryFilter>::sql_query(), data_filter.sql_query()),
//         };

//         query.distinct().take()
//     }

//     pub fn try_into_iter(
//         self,
//     ) -> Result<impl Iterator<Item = crate::Entity<'a>> + 'a, crate::Error> {
//         self.ecs.fetch(self.into_sql_query())
//     }

//     pub fn into_iter(self) -> impl Iterator<Item = crate::Entity<'a>> + 'a {
//         self.try_into_iter().unwrap()
//     }

//     pub fn into_data_iter<QD: QueryData + 'a>(self) -> impl Iterator<Item = QD::Output<'a>> + 'a {
//         self.try_into_data_iter::<QD>().unwrap()
//     }

//     pub fn try_into_data_iter<QD: QueryData + 'a>(
//         self,
//     ) -> Result<impl Iterator<Item = QD::Output<'a>> + 'a, crate::Error> {
//         Ok(self.try_into_iter()?.flat_map(|e| QD::from_entity(e)))
//     }
// }

impl QueryFilter for () {
    fn sql_query() -> sea_query::SelectStatement {
        use sea_query::*;
        Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .take()
    }
}

// impl QueryFilterValue for () {
//     fn sql_query(&self) -> sea_query::SelectStatement {
//         use sea_query::*;
//         Query::select()
//             .column(Components::Entity)
//             .from(Components::Table)
//             .take()
//     }
// }

// impl QueryFilterValue for EntityId {
//     fn sql_query(&self) -> sea_query::SelectStatement {
//         use sea_query::*;
//         Query::select()
//             .column(Components::Entity)
//             .from(Components::Table)
//             .and_where(Expr::col(Components::Entity).eq(*self))
//             .take()
//     }
// }

// impl<C: Component> QueryFilterValue for C {
//     fn sql_query(&self) -> sea_query::SelectStatement {
//         use sea_query::*;
//         let expr = match C::to_rusqlite(self).unwrap() {
//             rusqlite::types::Value::Blob(blob) => {
//                 struct Unhex;
//                 impl Iden for Unhex {
//                     fn unquoted(&self, s: &mut dyn Write) {
//                         write!(s, "unhex").unwrap();
//                     }
//                 }

//                 Expr::col(Components::Data).eq(Func::cust(Unhex).arg(hex::encode_upper(blob)))
//             }
//             rusqlite::types::Value::Text(json) => Expr::col(Components::Data).eq(json),
//             rusqlite::types::Value::Null => Expr::col(Components::Data).is_null(),
//             other => unreachable!("{other:?}"),
//         };
//         <Self as Filter>::sql_query().and_where(expr).take()
//     }
// }

// impl<C: Component> Filter for C {
//     fn sql_query() -> sea_query::SelectStatement {
//         use sea_query::*;
//         sea_query::Query::select()
//             .column(Components::Entity)
//             .from(Components::Table)
//             .and_where(Expr::col(Components::Component).eq(C::component_name()))
//             .take()
//     }
// }

pub struct With<C>(PhantomData<C>);
pub struct Without<C>(PhantomData<C>);
pub struct Or<F>(PhantomData<F>);

impl<C: Component> QueryFilter for With<C> {
    fn sql_query() -> sea_query::SelectStatement {
        use sea_query::*;
        Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .and_where(Expr::col(Components::Entity).in_subquery(<C as QueryData>::sql_query()))
            .take()
    }
}

impl<C: Component> QueryFilter for Without<C> {
    fn sql_query() -> sea_query::SelectStatement {
        use sea_query::*;
        Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .and_where(Expr::col(Components::Entity).not_in_subquery(<C as QueryData>::sql_query()))
            .take()
    }
}

pub trait QueryData {
    type Output<'a>: Sized;
    fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>>;
    fn sql_query() -> sea_query::SelectStatement;
}

impl QueryData for () {
    type Output<'a> = ();

    fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>> {
        Some(())
    }

    fn sql_query() -> sea_query::SelectStatement {
        sea_query::Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .take()
    }
}

impl QueryData for Entity<'_> {
    type Output<'a> = Entity<'a>;

    fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>> {
        Some(e)
    }

    fn sql_query() -> sea_query::SelectStatement {
        sea_query::Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .take()
    }
}

impl QueryData for EntityId {
    type Output<'a> = EntityId;

    fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>> {
        Some(e.id())
    }

    fn sql_query() -> sea_query::SelectStatement {
        sea_query::Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .take()
    }
}

impl<C: Component> QueryData for C {
    type Output<'a> = C;

    fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>> {
        e.component::<C>()
    }

    fn sql_query() -> sea_query::SelectStatement {
        use sea_query::*;

        sea_query::Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .and_where(Expr::col(Components::Component).eq(C::component_name()))
            .take()
    }
}

macro_rules! tuple_impl {
    ($t:tt, $($ts:tt),+) => {
        impl<$t, $($ts,)+> QueryData for ($t, $($ts,)+)
        where
            $t: QueryData,
            $($ts: QueryData,)+
        {
            type Output<'a> = ($t::Output<'a>, $($ts::Output<'a>,)+);

            fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>> {
                Some(($t::from_entity(e)?, $($ts::from_entity(e)?,)+))
            }

            fn sql_query() -> sea_query::SelectStatement {
                intersection($t::sql_query(), <($($ts,)+)>::sql_query())
            }
        }

        impl<$t, $($ts,)+> QueryFilter for ($t, $($ts,)+)
        where
            $t: QueryFilter,
            $($ts: QueryFilter,)+
        {
            fn sql_query() -> sea_query::SelectStatement {
                intersection($t::sql_query(), <($($ts,)+)>::sql_query())
            }
        }

        impl<$t, $($ts,)+> QueryFilter for Or<($t, $($ts,)+)>
        where
            $t: QueryFilter,
            $($ts: QueryFilter,)+
        {
            fn sql_query() -> sea_query::SelectStatement {
                union($t::sql_query(), <Or<($($ts,)+)>>::sql_query())
            }
        }

        impl<$t, $($ts,)+> QueryFilter for Without<($t, $($ts,)+)>
        where
            $t: Component,
            $($ts: Component,)+
        {
            fn sql_query() -> sea_query::SelectStatement {
                intersection(Without::<$t>::sql_query(), Without::<($($ts,)+)>::sql_query())
            }
        }


        impl<$t, $($ts,)+> QueryFilter for With<($t, $($ts,)+)>
        where
            $t: Component,
            $($ts: Component,)+
        {
            fn sql_query() -> sea_query::SelectStatement {
                intersection(With::<$t>::sql_query(), With::<($($ts,)+)>::sql_query())
            }
        }

        // impl<$t, $($ts,)+> QueryFilterValue for ($t, $($ts,)+)
        // where
        //     $t: QueryFilterValue,
        //     $($ts: QueryFilterValue,)+
        // {
        //     fn sql_query(&self) -> sea_query::SelectStatement {
        //         // and(self.0.sql_query(), self.1.sql_query())
        //         todo!()
        //     }
        // }

        // impl<$t, $($ts,)+> QueryData for ($t, $($ts,)+)
        // where
        //     $t: QueryData,
        //     $($ts: QueryData,)+
        // {
        //     type Output<'a> = ($t::Output<'a>, $($ts::Output<'a>,)+);
        //     fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>> {
        //         Some(($t::from_entity(e)?, $($ts::from_entity(e)?,)+))
        //     }
        // }


        tuple_impl!($($ts),+);
    };
    ($t:tt) => {
        impl<$t: QueryData> QueryData for ($t,) {
            type Output<'a> = ($t::Output<'a>,);

            fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>> {
                Some(($t::from_entity(e)?,))
            }

            fn sql_query() -> sea_query::SelectStatement {
                $t::sql_query().take()
            }
        }

        impl<$t: Filter> Filter for Or<($t,)> {
            fn sql_query() -> sea_query::SelectStatement {
                $t::sql_query().take()
            }
        }

        impl<$t: QueryFilter> QueryFilter for Or<($t,)> {
            fn sql_query() -> sea_query::SelectStatement {
                $t::sql_query().take()
            }
        }


        impl<$t: Component> QueryFilter for With<($t,)> {
            fn sql_query() -> sea_query::SelectStatement {
                With::<$t>::sql_query().take()
            }
        }
        impl<$t: Component> QueryFilter for Without<($t,)> {
            fn sql_query() -> sea_query::SelectStatement {
                Without::<$t>::sql_query().take()
            }
        }

        // impl<$t: Component> QueryFilterValue for ($t,) {
        //     fn sql_query(&self) -> sea_query::SelectStatement {
        //         // <$t as QueryFilterValue>::sql_query().take()
        //         todo!()
        //     }
        // }

        // impl<$t: DataFilter> DataFilter for ($t,) {
        //     fn sql_query(self) -> sea_query::SelectStatement {
        //         self.0.sql_query()
        //     }
        // }
    };
}

tuple_impl!(A, B, C, D, E, F, G, H, I, J, K, L, M, O, P, Q);
// tuple_impl!(A, B, C);

fn intersection(
    a: sea_query::SelectStatement,
    b: sea_query::SelectStatement,
) -> sea_query::SelectStatement {
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

fn union(
    a: sea_query::SelectStatement,
    b: sea_query::SelectStatement,
) -> sea_query::SelectStatement {
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
