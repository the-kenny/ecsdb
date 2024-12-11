use std::{any::Any, path::Path};

use query::ValueFilter;
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
        conn.pragma_update(None, "journal_mode", "wal")?;
        conn.execute_batch(include_str!("schema.sql"))?;
        Ok(Self { conn })
    }
}

impl Ecs {
    pub fn close(self) -> Result<(), Error> {
        self.conn.close().map_err(|(_conn, e)| Error::Database(e))
    }

    pub fn data_version(&self) -> Result<i64, Error> {
        Ok(self
            .conn
            .query_row("select data_version from pragma_data_version", [], |x| {
                x.get("data_version")
            })?)
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

pub type EntityId = i64;

impl Ecs {
    pub fn new_entity<'a>(&'a self) -> GenericEntity<'a, WithoutEntityId> {
        GenericEntity(&self, WithoutEntityId)
    }

    pub fn entity<'a>(&'a self, eid: EntityId) -> Entity<'a> {
        GenericEntity(&self, WithEntityId(eid))
    }
}

impl Ecs {
    pub fn query<'a, F>(&'a self) -> impl Iterator<Item = Entity<'a>> + 'a
    where
        F: query::Filter + 'a,
    {
        self.try_query::<'a, F>().unwrap()
    }

    pub fn try_query<'a, F>(&'a self) -> Result<impl Iterator<Item = Entity<'a>> + 'a, Error>
    where
        F: query::Filter + 'a,
    {
        let _span = debug_span!("query").entered();

        debug!("Running Query {}", std::any::type_name::<F>());
        self.fetch(query::Query::<F, ()>::new(()))
    }
}

impl Ecs {
    pub fn find<'a, V: ValueFilter>(
        &'a self,
        components: V,
    ) -> impl Iterator<Item = Entity<'a>> + 'a {
        self.try_find(components).unwrap()
    }

    pub fn try_find<'a, V: ValueFilter>(
        &'a self,
        components: V,
    ) -> Result<impl Iterator<Item = Entity<'a>> + 'a, Error> {
        let _span = debug_span!("try_find").entered();

        self.fetch(query::Query::<(), _>::new(components))
    }
}

impl Ecs {
    fn fetch<'a, F, V>(
        &'a self,
        query: query::Query<F, V>,
    ) -> Result<impl Iterator<Item = Entity<'a>> + 'a, Error>
    where
        F: query::Filter,
        V: query::ValueFilter,
    {
        let sql = query.sql_query().to_string(sea_query::SqliteQueryBuilder);
        println!("{sql}");
        debug!(sql);

        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, EntityId>("entity"))?
                .map(|r| r.expect("Valid EntityId"));
            rows.collect::<Vec<_>>()
        };

        debug!("Found {} entities", rows.len());

        Ok(rows.into_iter().scan(self, |conn, eid| {
            Some(GenericEntity(&conn, WithEntityId(eid)))
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
    use serde::Serialize;

    use crate::EntityId;

    use super::{sql::Components, ComponentName};
    use std::marker::PhantomData;

    pub trait Filter {
        fn sql_query() -> sea_query::SelectStatement;
    }

    pub trait ValueFilter: Serialize + Sized {
        fn sql_query(self) -> sea_query::SelectStatement;
    }

    impl ValueFilter for () {
        fn sql_query(self) -> sea_query::SelectStatement {
            <Self as Filter>::sql_query()
        }
    }

    impl ValueFilter for EntityId {
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

    impl<C: ComponentName + Serialize> ValueFilter for C {
        fn sql_query(self) -> sea_query::SelectStatement {
            use sea_query::*;
            let json = serde_json::to_string(&self).unwrap();
            <Self as Filter>::sql_query()
                .and_where(Expr::col(Components::Data).eq(json))
                .take()
        }
    }

    pub(crate) struct Query<F: ?Sized, C>(C, PhantomData<F>);

    impl<F, C> Query<F, C> {
        pub fn new(components: C) -> Self {
            Self(components, PhantomData::default())
        }
    }

    impl<F, C> Query<F, C>
    where
        F: Filter,
        C: ValueFilter,
    {
        pub(crate) fn sql_query(self) -> sea_query::SelectStatement {
            and(<F as Filter>::sql_query(), self.0.sql_query())
                .distinct()
                .take()
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

    pub struct Without<C>(PhantomData<C>);
    impl<C: ComponentName> Filter for Without<C> {
        fn sql_query() -> sea_query::SelectStatement {
            use sea_query::*;
            Query::select()
                .column(Components::Entity)
                .from(Components::Table)
                .and_where(
                    Expr::col(Components::Entity).not_in_subquery(<C as Filter>::sql_query()),
                )
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
                $t: ComponentName,
                $($ts: ComponentName,)+
            {
                fn sql_query() -> sea_query::SelectStatement {
                    and(Without::<$t>::sql_query(), Without::<($($ts,)+)>::sql_query())
                }
            }

            impl<$t, $($ts,)+> ValueFilter for ($t, $($ts,)+)
            where
                $t: ValueFilter,
                $($ts: ValueFilter,)+
            {
                fn sql_query(self) -> sea_query::SelectStatement {
                    and(self.0.sql_query(), self.1.sql_query())
                }

                // fn component_jsons(self) -> Vec<(&'static str, serde_json::Value)> {
                //     self.1.component_jsons().into_iter().chain(<$t as ValueFilter>::component_jsons(self.0).into_iter()).collect()
                // }
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

            impl<$t: ComponentName> Filter for Without<($t,)> {
                fn sql_query() -> sea_query::SelectStatement {
                    Without::<$t>::sql_query().take()
                }
            }

            impl<$t: ValueFilter> ValueFilter for ($t,) {
                // fn component_jsons(self) -> Vec<(&'static str, serde_json::Value)> {
                //     <$t as ValueFilter>::component_jsons(self.0)
                // }
                fn sql_query(self) -> sea_query::SelectStatement {
                    self.0.sql_query()
                }
            }
        };
    }

    filter_tuple_impl!(A, B, C, D, E, F, G, H, I, J, K, L, M, O, P, Q);
    // filter_tuple_impl!(A, B, C);

    fn and(
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

    fn or(
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
}

#[derive(Debug, Copy, Clone)]
pub struct WithoutEntityId;
#[derive(Debug, Copy, Clone)]
pub struct WithEntityId(EntityId);

pub type Entity<'a> = GenericEntity<'a, WithEntityId>;
pub type NewEntity<'a> = GenericEntity<'a, WithoutEntityId>;

#[derive(Copy, Clone)]
pub struct GenericEntity<'a, S>(&'a Ecs, S);

impl<'a> GenericEntity<'a, WithEntityId> {
    pub fn id(&self) -> EntityId {
        (self.1).0
    }

    pub fn db(&self) -> &Ecs {
        self.0
    }

    pub fn component<T: ComponentName>(&self) -> Option<T> {
        self.try_component::<T>().unwrap()
    }

    pub fn try_component<T: ComponentName>(&self) -> Result<Option<T>, Error> {
        let name = T::component_name();
        let mut query = self
            .0
            .conn
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

impl<'a> GenericEntity<'a, WithEntityId> {
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

        self.0.conn.execute(
            "insert or replace into components (entity, component, data) values (?1, ?2, ?3)",
            params![self.id(), T::component_name(), json],
        )?;

        debug!(
            entity = self.id(),
            component = T::component_name(),
            "attached"
        );

        Ok(self)
    }

    pub fn try_detach<T: ComponentName>(self) -> Result<Self, Error> {
        self.0.conn.execute(
            "delete from components where entity = ?1 and component = ?2",
            params![self.id(), T::component_name()],
        )?;

        debug!(
            entity = self.id(),
            component = T::component_name(),
            "detached"
        );

        Ok(self)
    }

    pub fn try_destroy(self) -> Result<(), Error> {
        self.0
            .conn
            .execute("delete from components where entity = ?1", [])?;
        debug!(entity = self.id(), "destroyed");
        Ok(())
    }
}

impl<'a> GenericEntity<'a, WithoutEntityId> {
    pub fn attach<T: ComponentName>(self, component: T) -> GenericEntity<'a, WithEntityId> {
        self.try_attach::<T>(component).unwrap()
    }

    pub fn try_attach<T: ComponentName>(
        self,
        component: T,
    ) -> Result<GenericEntity<'a, WithEntityId>, Error> {
        let json =
            serde_json::to_string(&component).map_err(|e| Error::ComponentSave(e.to_string()))?;
        let eid = self.0.conn.query_row_and_then(
            r#"
            insert into components (entity, component, data) 
            values ((select coalesce(max(entity)+1, 100) from components), ?1, ?2) 
            returning entity
            "#,
            params![T::component_name(), json],
            |row| row.get::<_, EntityId>("entity"),
        )?;
        let entity = GenericEntity(self.0, WithEntityId(eid));

        debug!(
            entity = entity.id(),
            component = T::component_name(),
            "attached"
        );

        Ok(entity)
    }

    pub fn detach<T: ComponentName>(&mut self) -> &mut Self {
        self
    }

    pub fn try_detach<T: ComponentName>(&mut self) -> Result<&mut Self, Error> {
        Ok(self)
    }
}

impl<'a> std::fmt::Debug for GenericEntity<'a, WithoutEntityId> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Entity").field(&self.1).finish()
    }
}

impl<'a> std::fmt::Debug for GenericEntity<'a, WithEntityId> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Entity").field(&self.1).finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::ComponentName;
    use crate::{self as ecsdb, Ecs}; // #[derive(Component)] derives `impl ecsdb::ComponentName for ...`

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

    #[test]
    fn component_overwrites() {
        let db = super::Ecs::open_in_memory().unwrap();

        let entity = db
            .new_entity()
            .attach(ComponentWithData(42))
            .attach(ComponentWithData(23));
        assert_eq!(entity.component::<ComponentWithData>().unwrap().0, 23);
    }

    use super::query::*;

    #[test]
    fn queries() {
        let db = super::Ecs::open_in_memory().unwrap();
        let _ = db.query::<MarkerComponent>();
        let _ = db.query::<Without<(MarkerComponent, MarkerComponent)>>();
        let _ = db.query::<(
            MarkerComponent,
            Or<(
                Without<(MarkerComponent, MarkerComponent)>,
                (MarkerComponent, MarkerComponent),
                Or<(MarkerComponent, Without<MarkerComponent>)>,
            )>,
        )>();
        let _ = db.query::<(
            MarkerComponent,
            ComponentWithData,
            Without<(MarkerComponent, MarkerComponent)>,
        )>();
        let _ = db.query::<(MarkerComponent, Without<ComponentWithData>)>();
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

        assert_eq!(db.query::<()>().count(), 2);
        assert_eq!(db.query::<MarkerComponent>().count(), 1);
        assert_eq!(db.query::<MarkerComponent>().count(), 1);
        assert_eq!(db.query::<Without<MarkerComponent>>().count(), 1);
        assert_eq!(
            db.query::<(MarkerComponent, ComponentWithData)>().count(),
            1
        );
        assert_eq!(
            db.query::<(MarkerComponent, Without<MarkerComponent>)>()
                .count(),
            0
        );
        assert_eq!(
            db.query::<(
                MarkerComponent,
                Without<MarkerComponent>,
                Or<(MarkerComponent, ComponentWithData)>
            )>()
            .count(),
            0
        );
        assert_eq!(db.query::<ComponentWithData>().count(), 2);
    }

    #[test]
    fn find() {
        let db = Ecs::open_in_memory().unwrap();
        let eid = db.new_entity().attach(ComponentWithData(123)).id();
        let _ = db.new_entity().attach(ComponentWithData(123));
        let _ = db.new_entity().attach(ComponentWithData(255));

        assert_eq!(db.find(eid).count(), 1);
        assert_eq!(db.find(eid).next().unwrap().id(), eid);
        assert_eq!(db.find((eid, MarkerComponent)).count(), 0);
        assert_eq!(db.find(MarkerComponent).count(), 0);
        assert_eq!(db.find(ComponentWithData(0)).count(), 0);
        assert_eq!(db.find(ComponentWithData(123)).count(), 2);
        assert_eq!(db.find(ComponentWithData(255)).count(), 1);

        let _ = db
            .new_entity()
            .attach(MarkerComponent)
            .attach(ComponentWithData(12345));
        assert_eq!(
            db.find((MarkerComponent, ComponentWithData(12345))).count(),
            1
        );
    }

    #[test]
    fn enum_component() {
        #[derive(Serialize, Deserialize, Component, PartialEq, Debug)]
        enum Foo {
            A,
            B,
        }

        let db = Ecs::open_in_memory().unwrap();
        let entity = db.new_entity().attach(Foo::A);
        assert_eq!(entity.component::<Foo>().unwrap(), Foo::A);
    }
}
