use crate::{component::Bundle, Entity, EntityId};

use super::{sql::Components, Component};
use std::{borrow::Cow, marker::PhantomData};

pub trait QueryData {
    type Output<'a>: Sized;
    fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>>;
    fn sql_query() -> sea_query::SelectStatement;
}

pub trait QueryFilter {
    fn sql_query(&self) -> sea_query::SelectStatement;
}

pub struct With<C>(PhantomData<C>);
pub struct Without<C>(PhantomData<C>);
pub struct Or<F>(F);

pub trait FilterValue: Sized {
    fn sql_query(&self) -> sea_query::SelectStatement;
}

pub struct Query<'a, D = Entity<'a>, F = ()>
where
    F: ?Sized,
{
    pub(crate) ecs: &'a crate::Ecs,
    pub(crate) data: PhantomData<D>,
    pub(crate) filter: F,
}

impl<'a, C, F> Query<'a, C, F> {
    pub fn new(ecs: &'a crate::Ecs, filter: F) -> Self {
        Self {
            ecs,
            data: PhantomData,
            filter,
        }
    }
}

impl<'a, D, F> Query<'a, D, F>
where
    D: QueryData + 'a,
    F: QueryFilter,
{
    pub fn iter(&self) -> impl Iterator<Item = D::Output<'a>> + 'a {
        self.try_iter().unwrap()
    }

    pub fn reverse_iter(&self) -> impl Iterator<Item = D::Output<'a>> + 'a {
        self.try_reverse_iter().unwrap()
    }

    pub fn entities(&self) -> impl Iterator<Item = Entity<'a>> + 'a {
        self.try_entities().unwrap()
    }

    pub fn reverse_entities(&self) -> impl Iterator<Item = Entity<'a>> + 'a {
        self.try_reverse_entities().unwrap()
    }

    pub fn try_iter(&self) -> Result<impl Iterator<Item = D::Output<'a>> + 'a, crate::Error> {
        Ok(self.try_entities()?.filter_map(|e| D::from_entity(e)))
    }

    pub fn try_reverse_iter(
        &self,
    ) -> Result<impl Iterator<Item = D::Output<'a>> + 'a, crate::Error> {
        Ok(self
            .try_reverse_entities()?
            .filter_map(|e| D::from_entity(e)))
    }

    pub fn try_entities(&self) -> Result<impl Iterator<Item = Entity<'a>> + 'a, crate::Error> {
        let mut query = self.as_sql_query();
        query.order_by(Components::Entity, sea_query::Order::Asc);
        self.ecs.fetch::<Entity>(query)
    }

    pub fn try_reverse_entities(
        &self,
    ) -> Result<impl Iterator<Item = Entity<'a>> + 'a, crate::Error> {
        let mut query = self.as_sql_query();
        query.order_by(Components::Entity, sea_query::Order::Desc);
        self.ecs.fetch::<Entity>(query)
    }

    fn as_sql_query(&self) -> sea_query::SelectStatement {
        intersection(
            <D as QueryData>::sql_query(),
            <F as QueryFilter>::sql_query(&self.filter),
        )
        .distinct()
        .take()
    }
}

impl QueryData for () {
    type Output<'a> = ();

    fn from_entity<'a>(_e: Entity<'a>) -> Option<Self::Output<'a>> {
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

impl QueryFilter for () {
    fn sql_query(&self) -> sea_query::SelectStatement {
        use sea_query::*;
        Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .take()
    }
}

impl<C: Bundle> Default for With<C> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<C: Bundle> Default for Without<C> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<F: QueryFilter + Default> Default for Or<F> {
    fn default() -> Self {
        Self(F::default())
    }
}

impl<C: Component> QueryFilter for C {
    fn sql_query(&self) -> sea_query::SelectStatement {
        <C as QueryData>::sql_query()
    }
}

impl<C: Component> QueryFilter for With<C> {
    fn sql_query(&self) -> sea_query::SelectStatement {
        <C as QueryData>::sql_query()
    }
}

impl<C: Component> QueryFilter for Without<C> {
    fn sql_query(&self) -> sea_query::SelectStatement {
        use sea_query::*;
        Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .and_where(Expr::col(Components::Entity).not_in_subquery(<C as QueryData>::sql_query()))
            .take()
    }
}

pub(crate) struct FilterValueWrapper<F: Sized + FilterValue>(pub(crate) F);

impl<F: FilterValue> QueryFilter for FilterValueWrapper<F> {
    fn sql_query(&self) -> sea_query::SelectStatement {
        <F as FilterValue>::sql_query(&self.0)
    }
}

impl FilterValue for EntityId {
    fn sql_query(&self) -> sea_query::SelectStatement {
        use sea_query::*;
        Query::select()
            .column(Components::Entity)
            .from(Components::Table)
            .and_where(Expr::col(Components::Entity).eq(*self))
            .take()
    }
}

impl<C: Component> FilterValue for C {
    fn sql_query(&self) -> sea_query::SelectStatement {
        use rusqlite::types::ToSqlOutput;
        use sea_query::*;
        let data = C::to_rusqlite(self).unwrap();

        enum Value<'a> {
            Null,
            Blob(Cow<'a, [u8]>),
            Text(Cow<'a, [u8]>),
        }

        let value = match data {
            ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Null) => Value::Null,
            ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Blob(b)) => {
                Value::Blob(Cow::Borrowed(b))
            }
            ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Text(s)) => {
                Value::Text(Cow::Borrowed(s))
            }
            ToSqlOutput::Owned(rusqlite::types::Value::Null) => Value::Null,
            ToSqlOutput::Owned(rusqlite::types::Value::Blob(b)) => Value::Blob(Cow::Owned(b)),
            ToSqlOutput::Owned(rusqlite::types::Value::Text(s)) => {
                Value::Text(Cow::Owned(s.into_bytes()))
            }
            other => unreachable!("{other:?}"),
        };

        let expr = match value {
            Value::Blob(blob) => {
                struct Unhex;
                impl Iden for Unhex {
                    fn unquoted(&self, s: &mut dyn Write) {
                        write!(s, "unhex").unwrap();
                    }
                }

                Expr::col(Components::Data).eq(Func::cust(Unhex).arg(hex::encode_upper(blob)))
            }
            Value::Text(json) => {
                Expr::col(Components::Data).eq(String::from_utf8(json.into_owned()).expect("utf8"))
            }
            Value::Null => Expr::col(Components::Data).is_null(),
        };

        <Self as QueryFilter>::sql_query(self)
            .and_where(expr)
            .take()
    }
}

mod tuples {
    use super::*;

    macro_rules! query_data_impl {
        ( $($ts:ident)* ) => {
            impl<$($ts,)+> QueryData for ($($ts,)+)
            where
                $($ts: QueryData,)+
            {
                type Output<'a> = ($($ts::Output<'a>,)+);

                fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>> {
                    Some(($($ts::from_entity(e)?,)+))
                }

                fn sql_query() -> sea_query::SelectStatement {
                    [
                        $(<$ts as QueryData>::sql_query()),+
                    ].into_iter().reduce(intersection).unwrap()
                }
            }
        }
    }

    macro_rules! filter_value_impl {
        ( $($ts:ident)* ) => {

            impl<$($ts,)+> FilterValue for ($($ts,)+)
            where
                $($ts: FilterValue,)+
            {
                fn sql_query(&self) -> sea_query::SelectStatement{
                    #[allow(non_snake_case)]
                    let ($($ts,)+) = self;
                    [
                        $($ts.sql_query(),)+
                    ].into_iter().reduce(intersection).unwrap()
                }
            }
        }
    }

    macro_rules! impl_query_filter {
        ( $($ts:ident)* ) => {
            impl<$($ts,)+> QueryFilter for ($($ts,)+)
            where
                $($ts: QueryFilter,)+
            {
                fn sql_query(&self) -> sea_query::SelectStatement{
                    #[allow(non_snake_case)]
                    let ($($ts,)+) = self;
                    [
                        $($ts.sql_query(),)+
                    ].into_iter().reduce(intersection).unwrap()
                }
            }

            impl<$($ts,)+> QueryFilter for Or<($($ts,)+)>
            where
                $($ts: QueryFilter,)+
            {
                fn sql_query(&self) -> sea_query::SelectStatement{
                    #[allow(non_snake_case)]
                    let Or(($($ts,)+)) = self;
                    [
                        $($ts.sql_query(),)+
                    ].into_iter().reduce(union).unwrap()
                }
            }

            impl<$($ts,)+> QueryFilter for With <($($ts,)+)>
            where
                $($ts: Component,)+
            {
                fn sql_query(&self) -> sea_query::SelectStatement{
                    #[allow(non_snake_case)]
                    [
                        $(With::<$ts>::sql_query(&Default::default()),)+
                    ].into_iter().reduce(intersection).unwrap()
                }
            }

            impl<$($ts,)+> QueryFilter for Without <($($ts,)+)>
            where
                $($ts: Component,)+
            {
                fn sql_query(&self) -> sea_query::SelectStatement{
                    #[allow(non_snake_case)]
                    [
                        $(Without::<$ts>::sql_query(&Default::default()),)+
                    ].into_iter().reduce(intersection).unwrap()
                }
            }
        };
    }

    crate::tuple_macros::for_each_tuple!(query_data_impl);
    crate::tuple_macros::for_each_tuple!(filter_value_impl);
    crate::tuple_macros::for_each_tuple!(impl_query_filter);
}

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
