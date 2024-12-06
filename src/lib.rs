use std::{any::Any, path::Path};

use rusqlite::params;
use serde::{de::DeserializeOwned, Serialize};
use tracing::{debug, debug_span};

pub use ecsdb_derive::Component;

pub struct Ecs {
    conn: rusqlite::Connection,
}

impl Ecs {
    pub fn open_in_memory() -> Result<Self, Error> {
        Self::from_rusqlite(rusqlite::Connection::open_in_memory()?)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::from_rusqlite(rusqlite::Connection::open(path)?)
    }

    pub fn from_rusqlite(conn: rusqlite::Connection) -> Result<Self, Error> {
        conn.execute_batch(include_str!("schema.sql"))?;
        Ok(Self { conn })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Database Error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Failed to load Component: {0}")]
    ComponentLoad(String),
    #[error("Failed to save Component: {0}")]
    ComponentSave(String),
}

pub trait ComponentName: Serialize + DeserializeOwned + Any {
    fn component_name() -> &'static str;
}

type EntityId = i64;

impl Ecs {
    pub fn new_entity<'a>(&'a self) -> Entity<'a, WithoutEntityId> {
        Entity(&self.conn, WithoutEntityId)
    }

    pub fn entity<'a>(&'a self, eid: EntityId) -> Entity<'a, WithEntityId> {
        Entity(&self.conn, WithEntityId(eid))
    }
}

impl Ecs {
    pub fn query<'a, Q>(&'a self) -> impl Iterator<Item = Entity<'a, WithEntityId>> + 'a
    where
        Q: query::IntoQuery + 'a,
    {
        self.try_query::<'a, Q>().unwrap()
    }

    pub fn try_query<'a, Q>(
        &'a self,
    ) -> Result<impl Iterator<Item = Entity<'a, WithEntityId>> + 'a, Error>
    where
        Q: query::IntoQuery + 'a,
    {
        let _span = debug_span!("query").entered();

        debug!("Running Query {}", std::any::type_name::<Q>());

        let q = Q::new();

        use sea_query::*;
        let sql = q.filter_query().to_string(SqliteQueryBuilder);
        debug!(%sql);

        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, EntityId>("entity"))?
                .map(|r| r.expect("Valid EntityId"));
            rows.collect::<Vec<_>>()
        };

        debug!("Found {} entities", rows.len());

        Ok(rows.into_iter().scan(&self.conn, |conn, eid| {
            Some(Entity(&conn, WithEntityId(eid)))
        }))
    }
}

mod sql {
    #[allow(unused)]
    pub enum Components {
        Table,
        Entity,
        Component,
        Data,
    }

    impl sea_query::Iden for Components {
        fn unquoted(&self, s: &mut dyn std::fmt::Write) {
            let v = match self {
                Components::Table => "components",
                Components::Entity => "entity",
                Components::Component => "component",
                Components::Data => "data",
            };
            write!(s, "{v}").unwrap()
        }
    }
}

pub mod query {
    use super::{sql::Components, ComponentName};
    use std::marker::PhantomData;

    pub trait Filter {
        fn sql_query() -> sea_query::SelectStatement;
    }

    pub struct Query<F>(PhantomData<F>);

    impl<F> Query<F>
    where
        F: Filter,
    {
        // pub fn component_names(&self) -> impl Iterator<Item = &'static str> {
        //     iter::once(C::component_name())
        // }

        pub fn filter_query(&self) -> sea_query::SelectStatement {
            F::sql_query()
        }
    }

    pub trait IntoQuery {
        type F: Filter;

        fn new() -> Query<Self::F>;
    }

    impl<F> IntoQuery for Query<F>
    where
        F: Filter,
    {
        type F = F;

        fn new() -> Self {
            Self(Default::default())
        }
    }

    impl<F: Filter> IntoQuery for F {
        type F = F;

        fn new() -> Query<Self::F> {
            Query(Default::default())
        }
    }

    impl<C: ComponentName> Filter for C {
        fn sql_query() -> sea_query::SelectStatement {
            use sea_query::*;
            sea_query::Query::select()
                .column(Components::Entity)
                .from(Components::Table)
                .and_where(Expr::col(Components::Component).eq(C::component_name()))
                .take()
        }
    }

    pub struct With<F>(PhantomData<F>);
    impl<F: Filter> Filter for With<F> {
        fn sql_query() -> sea_query::SelectStatement {
            F::sql_query()
        }
    }

    pub struct Without<F>(PhantomData<F>);
    impl<F: Filter> Filter for Without<F> {
        fn sql_query() -> sea_query::SelectStatement {
            use sea_query::*;
            Query::select()
                .column(Components::Entity)
                .from(Components::Table)
                .and_where(Expr::col(Components::Entity).not_in_subquery(F::sql_query()))
                .take()
        }
    }

    pub struct And<A, B>(PhantomData<(A, B)>);
    impl<A, B> Filter for And<A, B>
    where
        A: Filter,
        B: Filter,
    {
        fn sql_query() -> sea_query::SelectStatement {
            A::sql_query()
                .union(sea_query::UnionType::Intersect, B::sql_query())
                .take()
        }
    }

    pub struct Or<A, B>(PhantomData<(A, B)>);
    impl<A, B> Filter for Or<A, B>
    where
        A: Filter,
        B: Filter,
    {
        fn sql_query() -> sea_query::SelectStatement {
            A::sql_query()
                .union(sea_query::UnionType::All, B::sql_query())
                .take()
        }
    }

    macro_rules! filter_tuple_impl {
        ($t:tt) => {
            impl<$t> Filter for ($t,)
            where
                $t: Filter,
            {
                fn sql_query() -> sea_query::SelectStatement {
                    $t::sql_query().take()
                    // let queries = [
                    //     $(
                    //         $t::sql_query().take(),
                    //     )+
                    // ];
                    // queries.into_iter().reduce(|mut a,b| a.union(sea_query::UnionType::Intersect, b).take()).unwrap()
                }
            }
        };
        ($t:tt, $($ts:tt),+) => {
            impl<$t, $($ts,)+> Filter for ($t, $($ts,)+)
            where
                $t: Filter,
                $($ts: Filter,)+
            {
                fn sql_query() -> sea_query::SelectStatement {
                    $t::sql_query().union(sea_query::UnionType::Intersect, <($($ts,)+) as Filter>::sql_query()).take()
                }
            }

            filter_tuple_impl!($($ts),+);
        };

    }

    filter_tuple_impl!(A, B, C, D, E, F, G, H, I, J, K, L, M, O, P, Q, R, S, T, U, V, W, X, Y, Z);

    // impl<F1, F2> Filter for (F1, F2)
    // where
    //     F1: Filter,
    //     F2: Filter,
    // {
    //     fn sql_query() -> sea_query::SelectStatement {
    //         F1::sql_query()
    //             .union(sea_query::UnionType::Intersect, F2::sql_query())
    //             .take()
    //     }
    // }
}

#[derive(Debug, Copy, Clone)]
pub struct WithoutEntityId;
#[derive(Debug, Copy, Clone)]
pub struct WithEntityId(EntityId);
#[derive(Copy, Clone)]
pub struct Entity<'a, S>(&'a rusqlite::Connection, S);

impl<'a> Entity<'a, WithEntityId> {
    pub fn id(&self) -> EntityId {
        (self.1).0
    }

    pub fn component<T: ComponentName>(&self) -> Option<T> {
        self.try_component::<T>().unwrap()
    }

    pub fn try_component<T: ComponentName>(&self) -> Result<Option<T>, Error> {
        let name = std::any::type_name::<T>();
        let mut query = self
            .0
            .prepare("select data from components where entity = ?1 and component = ?2")?;
        let row = query
            .query_and_then(params![self.id(), name], |row| row.get::<_, String>("data"))?
            .next();

        match row {
            None => Ok(None),
            Some(Ok(data)) => {
                let component =
                    serde_json::from_str(&data).map_err(|e| Error::ComponentLoad(e.to_string()))?;
                Ok(Some(component))
            }
            _other => panic!(),
        }
    }
}

impl<'a> Entity<'a, WithEntityId> {
    pub fn attach<T: ComponentName>(self, component: T) -> Self {
        self.try_attach::<T>(component).unwrap()
    }

    pub fn detach<T: ComponentName>(self) -> Self {
        self.try_detach::<T>().unwrap()
    }

    pub fn destroy(self) {
        self.try_destroy().unwrap();
    }

    pub fn try_attach<T: ComponentName>(self, component: T) -> Result<Self, Error> {
        let json =
            serde_json::to_string(&component).map_err(|e| Error::ComponentSave(e.to_string()))?;

        self.0.query_row_and_then(
            "insert into components (entity, component, data) values (?1, ?2, ?3) returning entity",
            params![self.id(), T::component_name(), json],
            |row| row.get::<_, EntityId>("entity"),
        )?;

        Ok(self)
    }

    pub fn try_detach<T: ComponentName>(self) -> Result<Self, Error> {
        self.0.execute(
            "delete from components where entity = ?1 and component = ?2",
            params![self.id(), T::component_name()],
        )?;

        Ok(self)
    }

    pub fn try_destroy(self) -> Result<(), Error> {
        self.0
            .execute("delete from components where entity = ?1", [])?;
        Ok(())
    }
}

impl<'a> Entity<'a, WithoutEntityId> {
    pub fn attach<T: ComponentName>(self, component: T) -> Entity<'a, WithEntityId> {
        self.try_attach::<T>(component).unwrap()
    }

    pub fn try_attach<T: ComponentName>(
        self,
        component: T,
    ) -> Result<Entity<'a, WithEntityId>, Error> {
        let json =
            serde_json::to_string(&component).map_err(|e| Error::ComponentSave(e.to_string()))?;
        let eid = self.0.query_row_and_then(
            r#"
            insert into components (entity, component, data) 
            values ((select coalesce(max(entity)+1, 100) from components), ?1, ?2) 
            returning entity
            "#,
            params![T::component_name(), json],
            |row| row.get::<_, EntityId>("entity"),
        )?;
        Ok(Entity(self.0, WithEntityId(eid)))
    }

    pub fn detach<T: ComponentName>(&mut self) -> &mut Self {
        self
    }

    pub fn try_detach<T: ComponentName>(&mut self) -> Result<&mut Self, Error> {
        Ok(self)
    }
}

impl<'a> std::fmt::Debug for Entity<'a, WithoutEntityId> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Entity").field(&self.1).finish()
    }
}

impl<'a> std::fmt::Debug for Entity<'a, WithEntityId> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Entity").field(&self.1).finish()
    }
}

#[cfg(test)]
mod tests {
    use crate as ecsdb; // #[derive(Component)] derives `impl ecsdb::ComponentName for ...`
    use crate::ComponentName;

    use ecsdb_derive::Component;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct MarkerComponent;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Component)]
    struct ComponentWithData(u64);

    #[test]
    fn derive_valid_component_name() {
        assert_eq!(
            MarkerComponent::component_name(),
            "ecsdb::tests::MarkerComponent"
        );
        assert_eq!(
            ComponentWithData::component_name(),
            "ecsdb::tests::ComponentWithData"
        );
    }

    #[test]
    fn entity_attach_detach() {
        let db = super::Ecs::open_in_memory().unwrap();
        let entity = db
            .new_entity()
            .attach(ComponentWithData(1234))
            .attach(MarkerComponent);

        assert!(entity.component::<MarkerComponent>().is_some());
        entity.detach::<MarkerComponent>();
        assert!(entity.component::<MarkerComponent>().is_none());

        assert_eq!(
            entity.component::<ComponentWithData>(),
            Some(ComponentWithData(1234))
        );
    }

    use super::query::*;

    #[test]
    fn queries() {
        let db = super::Ecs::open_in_memory().unwrap();
        let _ = db.query::<MarkerComponent>();
        let _ = db.query::<With<MarkerComponent>>();
        let _ = db.query::<And<MarkerComponent, Without<ComponentWithData>>>();
        let _ = db.query::<(MarkerComponent, Without<ComponentWithData>)>();
        let _ = db.query::<(
            MarkerComponent,
            MarkerComponent,
            MarkerComponent,
            MarkerComponent,
            MarkerComponent,
            MarkerComponent,
            MarkerComponent,
            MarkerComponent,
        )>();
    }

    #[test]
    fn query() {
        let db = super::Ecs::open_in_memory().unwrap();

        db.new_entity()
            .attach(MarkerComponent)
            .attach(ComponentWithData(1234));

        db.new_entity().attach(ComponentWithData(1234));

        for entity in db.query::<With<MarkerComponent>>() {
            dbg!(entity);
        }

        for entity in db.query::<MarkerComponent>() {
            dbg!(entity);
        }

        for entity in db.query::<Query<Without<MarkerComponent>>>() {
            dbg!(entity);
        }

        for entity in db.query::<Query<And<With<MarkerComponent>, With<ComponentWithData>>>>() {
            dbg!(entity);
        }
    }
}
