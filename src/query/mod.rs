use tracing::trace;

use crate::{component::Bundle, Entity, EntityId};

use super::Component;
use std::marker::PhantomData;

pub(crate) mod ir;

pub trait QueryData {
    type Output<'a>: Sized;
    fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>>;
    fn filter_expression() -> ir::FilterExpression;
}

pub trait QueryFilter {
    fn filter_expression() -> ir::FilterExpression;
}

/// Matches if any of the Filters in `C` matches
pub struct AnyOf<C>(PhantomData<C>);

/// Matches if Entity has all components in `C`
pub struct With<C>(PhantomData<C>);

/// Matches if Entity has none of the components in `C`
pub struct Without<C>(PhantomData<C>);

/// Matches if any of the filters in `F` match
pub struct Or<F>(F);

pub trait QueryFilterValue: Sized {
    fn filter_expression(&self) -> ir::FilterExpression;
}

pub struct Query<'a, D = Entity<'a>, F = (), V = ()>
where
    F: ?Sized,
{
    pub(crate) ecs: &'a crate::Ecs,
    pub(crate) data: PhantomData<D>,
    pub(crate) filter: PhantomData<F>,
    pub(crate) filter_value: V,
}

impl<'a, C, F> Query<'a, C, F, ()> {
    pub fn new(ecs: &'a crate::Ecs) -> Self {
        Self {
            ecs,
            data: PhantomData,
            filter: PhantomData,
            filter_value: (),
        }
    }
}

impl<'a, C, F, V> Query<'a, C, F, V> {
    pub fn with_filter(ecs: &'a crate::Ecs, filter_value: V) -> Self {
        Self {
            ecs,
            data: PhantomData,
            filter: PhantomData,
            filter_value,
        }
    }
}

impl<'a, D, F, V> Query<'a, D, F, V>
where
    D: QueryData + 'a,
    F: QueryFilter,
    V: QueryFilterValue,
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

        query.order_by = ir::OrderBy::Asc;
        self.ecs.fetch::<Entity>(query)
    }

    pub fn try_reverse_entities(
        &self,
    ) -> Result<impl Iterator<Item = Entity<'a>> + 'a, crate::Error> {
        let mut query = self.as_sql_query();
        query.order_by = ir::OrderBy::Desc;
        self.ecs.fetch::<Entity>(query)
    }

    #[tracing::instrument(level = "debug", skip_all)]
    fn as_sql_query(&self) -> ir::Query {
        let filter = ir::FilterExpression::and([
            D::filter_expression(),
            F::filter_expression(),
            self.filter_value.filter_expression(),
        ]);

        trace!(?filter);

        ir::Query {
            filter,
            order_by: ir::OrderBy::Asc,
        }
    }
}

impl QueryData for () {
    type Output<'a> = ();

    fn from_entity<'a>(_e: Entity<'a>) -> Option<Self::Output<'a>> {
        Some(())
    }

    fn filter_expression() -> ir::FilterExpression {
        ir::FilterExpression::none()
    }
}

impl QueryData for Entity<'_> {
    type Output<'a> = Entity<'a>;

    fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>> {
        Some(e)
    }

    fn filter_expression() -> ir::FilterExpression {
        ir::FilterExpression::none()
    }
}

impl QueryData for EntityId {
    type Output<'a> = EntityId;

    fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>> {
        Some(e.id())
    }

    fn filter_expression() -> ir::FilterExpression {
        ir::FilterExpression::none()
    }
}

impl<C: Component> QueryData for C {
    type Output<'a> = C;

    fn from_entity<'a>(e: Entity<'a>) -> Option<Self::Output<'a>> {
        e.component::<C>()
    }

    fn filter_expression() -> ir::FilterExpression {
        ir::FilterExpression::with_component(C::component_name())
    }
}

impl QueryFilter for () {
    fn filter_expression() -> ir::FilterExpression {
        ir::FilterExpression::none()
    }
}

impl<C: Bundle> Default for AnyOf<C> {
    fn default() -> Self {
        Self(PhantomData)
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
    fn filter_expression() -> ir::FilterExpression {
        ir::FilterExpression::with_component(C::component_name())
    }
}

impl<C: Bundle> QueryFilter for AnyOf<C> {
    fn filter_expression() -> ir::FilterExpression {
        ir::FilterExpression::or(
            C::component_names()
                .iter()
                .map(|c| ir::FilterExpression::with_component(c)),
        )
    }
}

impl<C: Component> QueryFilter for With<C> {
    fn filter_expression() -> ir::FilterExpression {
        ir::FilterExpression::with_component(C::component_name())
    }
}

impl<C: Component> QueryFilter for Without<C> {
    fn filter_expression() -> ir::FilterExpression {
        ir::FilterExpression::without_component(C::component_name())
    }
}

impl QueryFilterValue for () {
    fn filter_expression(&self) -> ir::FilterExpression {
        ir::FilterExpression::None
    }
}

impl QueryFilterValue for EntityId {
    fn filter_expression(&self) -> ir::FilterExpression {
        ir::FilterExpression::entity(*self)
    }
}

#[derive(PartialEq, Eq, Debug)]
pub struct ComponentName(pub String);

impl QueryFilterValue for ComponentName {
    fn filter_expression(&self) -> ir::FilterExpression {
        ir::FilterExpression::WithComponent(self.0.clone())
    }
}

impl<V: QueryFilterValue> QueryFilterValue for &[V] {
    fn filter_expression(&self) -> ir::FilterExpression {
        ir::FilterExpression::And(self.iter().map(V::filter_expression).collect())
    }
}

impl<C: Component> QueryFilterValue for C {
    fn filter_expression(&self) -> ir::FilterExpression {
        use rusqlite::types::ToSqlOutput;

        let value = match C::to_rusqlite(self).unwrap() {
            ToSqlOutput::Borrowed(v) => v.to_owned().into(),
            ToSqlOutput::Owned(v) => v,
            other => unreachable!("{other:?}"),
        };

        ir::FilterExpression::with_component_data(C::component_name(), value)
    }
}

impl<C: QueryFilterValue + Component> QueryFilterValue for std::ops::Range<C> {
    fn filter_expression(&self) -> ir::FilterExpression {
        use rusqlite::types::ToSqlOutput;

        let start = match C::to_rusqlite(&self.start).unwrap() {
            ToSqlOutput::Borrowed(v) => v.to_owned().into(),
            ToSqlOutput::Owned(v) => v,
            other => unreachable!("{other:?}"),
        };

        let end = match C::to_rusqlite(&self.end).unwrap() {
            ToSqlOutput::Borrowed(v) => v.to_owned().into(),
            ToSqlOutput::Owned(v) => v,
            other => unreachable!("{other:?}"),
        };

        ir::FilterExpression::WithComponentDataRange {
            component: C::component_name().to_owned(),
            start,
            end,
        }
    }
}

impl<C: QueryFilterValue + Component> QueryFilterValue for std::ops::RangeTo<C> {
    fn filter_expression(&self) -> ir::FilterExpression {
        use rusqlite::types::ToSqlOutput;

        let end = match C::to_rusqlite(&self.end).unwrap() {
            ToSqlOutput::Borrowed(v) => v.to_owned().into(),
            ToSqlOutput::Owned(v) => v,
            other => unreachable!("{other:?}"),
        };

        ir::FilterExpression::WithComponentDataRange {
            component: C::component_name().to_owned(),
            start: rusqlite::types::Value::Null,
            end,
        }
    }
}

impl<C: QueryFilterValue + Component> QueryFilterValue for std::ops::RangeFrom<C> {
    fn filter_expression(&self) -> ir::FilterExpression {
        use rusqlite::types::ToSqlOutput;

        let start = match C::to_rusqlite(&self.start).unwrap() {
            ToSqlOutput::Borrowed(v) => v.to_owned().into(),
            ToSqlOutput::Owned(v) => v,
            other => unreachable!("{other:?}"),
        };

        ir::FilterExpression::WithComponentDataRange {
            component: C::component_name().to_owned(),
            start,
            end: rusqlite::types::Value::Null,
        }
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


                fn filter_expression() -> ir::FilterExpression{
                    ir::FilterExpression::and([
                        $(<$ts as QueryData>::filter_expression()),+
                    ])
                }
            }
        }
    }

    macro_rules! filter_value_impl {
        ( $($ts:ident)* ) => {

            impl<$($ts,)+> QueryFilterValue for ($($ts,)+)
            where
                $($ts: QueryFilterValue,)+
            {

                fn filter_expression(&self) -> ir::FilterExpression{
                    #[allow(non_snake_case)]
                    let ($($ts,)+) = self;
                    ir::FilterExpression::and([
                        $($ts.filter_expression(),)+
                    ])
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

                #[allow(non_snake_case)]
                fn filter_expression() -> ir::FilterExpression{
                    ir::FilterExpression::and([
                        $(<$ts as QueryFilter>::filter_expression()),+
                    ])
                }
            }

            impl<$($ts,)+> QueryFilter for Or<($($ts,)+)>
            where
                $($ts: QueryFilter,)+
            {

                #[allow(non_snake_case)]
                fn filter_expression() -> ir::FilterExpression{
                    ir::FilterExpression::or([
                        $(<$ts as QueryFilter>::filter_expression()),+
                    ])
                }
            }

            impl<$($ts,)+> QueryFilter for With<($($ts,)+)>
            where
                $($ts: Component,)+
            {

                fn filter_expression() -> ir::FilterExpression{
                    ir::FilterExpression::and([
                        $(ir::FilterExpression::with_component($ts::component_name()),)+
                    ])
                }
            }

            impl<$($ts,)+> QueryFilter for Without<($($ts,)+)>
            where
                $($ts: Component,)+
            {

                fn filter_expression() -> ir::FilterExpression{
                    ir::FilterExpression::and([
                        $(ir::FilterExpression::without_component($ts::component_name()),)+
                    ])
                }
            }
        };
    }

    crate::tuple_macros::for_each_tuple!(query_data_impl);
    crate::tuple_macros::for_each_tuple!(filter_value_impl);
    crate::tuple_macros::for_each_tuple!(impl_query_filter);
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
