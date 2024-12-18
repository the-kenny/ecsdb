pub mod query;

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
    #[error(transparent)]
    ComponentStorage(#[from] StorageError),
}

pub trait Component: Sized + Any + ComponentRead<Self> + ComponentWrite<Self> {
    type Storage;

    fn component_name() -> &'static str;
}

pub trait ComponentWrite<C> {
    fn to_rusqlite(component: C) -> Result<rusqlite::types::Value, StorageError>;
}

pub trait ComponentRead<C> {
    fn from_rusqlite(value: rusqlite::types::Value) -> Result<C, StorageError>;
}

impl<C, S> ComponentRead<Self> for C
where
    C: Component<Storage = S>,
    S: ComponentRead<C>,
{
    fn from_rusqlite(value: rusqlite::types::Value) -> Result<Self, StorageError> {
        S::from_rusqlite(value)
    }
}

impl<C, S> ComponentWrite<Self> for C
where
    C: Component<Storage = S>,
    S: ComponentWrite<C>,
{
    fn to_rusqlite(component: Self) -> Result<rusqlite::types::Value, StorageError> {
        S::to_rusqlite(component)
    }
}

pub struct JsonStorage;

#[derive(thiserror::Error, Debug)]
#[error("Error storing Component: {0}")]
pub struct StorageError(String);

impl<C> ComponentRead<C> for JsonStorage
where
    C: Component + DeserializeOwned,
{
    fn from_rusqlite(value: rusqlite::types::Value) -> Result<C, StorageError> {
        match value {
            rusqlite::types::Value::Text(s) => {
                serde_json::from_str(&s).map_err(|e| StorageError(e.to_string()))
            }
            other => Err(StorageError(format!("Unexpected type {other:?}"))),
        }
    }
}

impl<C> ComponentWrite<C> for JsonStorage
where
    C: Component + Serialize,
{
    fn to_rusqlite(component: C) -> Result<rusqlite::types::Value, StorageError> {
        let json = serde_json::to_string(&component).map_err(|e| StorageError(e.to_string()))?;
        Ok(rusqlite::types::Value::Text(json))
    }
}

pub struct BlobStorage;

impl<C> ComponentRead<C> for BlobStorage
where
    C: Component + From<Vec<u8>>,
{
    fn from_rusqlite(value: rusqlite::types::Value) -> Result<C, StorageError> {
        match value {
            rusqlite::types::Value::Blob(b) => Ok(C::from(b)),
            other => Err(StorageError(format!("Unexpected type {other:?}"))),
        }
    }
}

impl<C> ComponentWrite<C> for BlobStorage
where
    C: Component + Into<Vec<u8>>,
{
    fn to_rusqlite(component: C) -> Result<rusqlite::types::Value, StorageError> {
        Ok(rusqlite::types::Value::Blob(component.into()))
    }
}

pub struct NullStorage;

impl<C> ComponentRead<C> for NullStorage
where
    C: Component + DeserializeOwned,
{
    fn from_rusqlite(value: rusqlite::types::Value) -> Result<C, StorageError> {
        match value {
            rusqlite::types::Value::Null => {
                serde_json::from_str("null").map_err(|e| StorageError(e.to_string()))
            }
            other => Err(StorageError(format!("Unexpected type {other:?}"))),
        }
    }
}

impl<C> ComponentWrite<C> for NullStorage
where
    C: Component + Serialize,
{
    fn to_rusqlite(_component: C) -> Result<rusqlite::types::Value, StorageError> {
        Ok(rusqlite::types::Value::Null)
    }
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

    pub fn component<T: Component>(&self) -> Option<T> {
        self.try_component::<T>().unwrap()
    }

    pub fn try_component<T: Component>(&self) -> Result<Option<T>, Error> {
        let name = T::component_name();
        let mut query = self
            .0
            .conn
            .prepare("select data from components where entity = ?1 and component = ?2")?;
        let row = query
            .query_and_then(params![self.id(), name], |row| {
                row.get::<_, rusqlite::types::Value>("data")
            })?
            .next();

        match row {
            None => Ok(None),
            Some(Ok(data)) => {
                let component = T::from_rusqlite(data)?;
                Ok(Some(component))
            }
            _other => panic!(),
        }
    }
}

impl<'a> GenericEntity<'a, WithEntityId> {
    pub fn attach<T: Component>(self, component: T) -> Self {
        self.try_attach::<T>(component).unwrap()
    }

    pub fn detach<T: Component>(self) -> Self {
        self.try_detach::<T>().unwrap()
    }

    pub fn destroy(self) {
        self.try_destroy().unwrap();
    }

    pub fn try_attach<T: Component>(self, component: T) -> Result<Self, Error> {
        let data = T::to_rusqlite(component)?;

        self.0.conn.execute(
            "insert or replace into components (entity, component, data) values (?1, ?2, ?3)",
            params![self.id(), T::component_name(), data],
        )?;

        debug!(
            entity = self.id(),
            component = T::component_name(),
            "attached"
        );

        Ok(self)
    }

    pub fn try_detach<T: Component>(self) -> Result<Self, Error> {
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
    pub fn attach<T: Component + ComponentWrite<T>>(
        self,
        component: T,
    ) -> GenericEntity<'a, WithEntityId> {
        self.try_attach::<T>(component).unwrap()
    }

    pub fn try_attach<T: Component + ComponentWrite<T>>(
        self,
        component: T,
    ) -> Result<GenericEntity<'a, WithEntityId>, Error> {
        let data = T::to_rusqlite(component)?;
        let eid = self.0.conn.query_row_and_then(
            r#"
            insert into components (entity, component, data) 
            values ((select coalesce(max(entity)+1, 100) from components), ?1, ?2) 
            returning entity
            "#,
            params![T::component_name(), data],
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

    pub fn detach<T: Component>(&mut self) -> &mut Self {
        self
    }

    pub fn try_detach<T: Component>(&mut self) -> Result<&mut Self, Error> {
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
    use crate::Component;
    use crate::{self as ecsdb, Ecs}; // #[derive(Component)] derives `impl ecsdb::Component for ...`

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

    #[test]
    fn blob_component() {
        #[derive(Component, Debug, PartialEq, Clone)]
        #[component(storage = "blob")]
        struct X(Vec<u8>);

        let x = X(b"asdfasdf".into());

        let db = Ecs::open_in_memory().unwrap();
        let entity = db.new_entity().attach(x.clone());

        assert_eq!(entity.component::<X>().unwrap(), x.clone());
        assert_eq!(db.find(x.clone()).next().unwrap().id(), entity.id());
    }
}
