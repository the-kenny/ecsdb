pub mod component;

pub use component::{Component, ComponentRead, ComponentWrite};

pub mod query;

mod system;
pub use system::*;

use std::{iter, path::Path};

use query::DataFilter;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

pub struct Ecs {
    conn: rusqlite::Connection,
    systems: Vec<Box<dyn System<(), ()>>>,
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
        Ok(Self {
            conn,
            systems: Default::default(),
        })
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
    ComponentStorage(#[from] component::StorageError),
}

pub type EntityId = i64;

impl Ecs {
    pub fn new_entity<'a>(&'a self) -> NewEntity<'a> {
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

    #[instrument(name = "query", level = "debug", skip_all)]
    pub fn try_query<'a, F>(&'a self) -> Result<impl Iterator<Item = Entity<'a>> + 'a, Error>
    where
        F: query::Filter + 'a,
    {
        debug!(query = std::any::type_name::<F>());
        let query = query::Query::<F, ()>::new(self, ());
        query.try_iter()
    }
}

impl Ecs {
    pub fn find<'a, V: DataFilter>(
        &'a self,
        components: V,
    ) -> impl Iterator<Item = Entity<'a>> + 'a {
        self.try_find(components).unwrap()
    }

    #[instrument(name = "find", level = "debug", skip_all)]
    pub fn try_find<'a, V: DataFilter>(
        &'a self,
        components: V,
    ) -> Result<impl Iterator<Item = Entity<'a>> + 'a, Error> {
        let query = query::Query::<(), _>::new(self, components);
        query.try_iter()
    }
}

impl Ecs {
    #[instrument(name = "fetch", level = "debug", skip_all)]
    fn fetch<'a>(
        &'a self,
        sql_query: sea_query::SelectStatement,
    ) -> Result<impl Iterator<Item = Entity<'a>> + 'a, Error> {
        // let sql = query.sql_query().to_string(sea_query::SqliteQueryBuilder);
        let sql = sql_query.to_string(sea_query::SqliteQueryBuilder);
        debug!(sql);

        let rows = {
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, EntityId>("entity"))?
                .map(|r| r.expect("Valid EntityId"));
            rows.collect::<Vec<_>>()
        };

        debug!(count = rows.len());

        Ok(rows.into_iter().scan(self, |conn, eid| {
            Some(GenericEntity(&conn, WithEntityId(eid)))
        }))
    }
}

use crate::{self as ecsdb};
#[derive(Component, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct BelongsTo(pub EntityId);

impl Ecs {
    fn direct_children<'a>(&'a self, entity: EntityId) -> impl Iterator<Item = Entity<'a>> + 'a {
        self.find(BelongsTo(entity))
    }

    fn all_children<'a>(&'a self, entity: EntityId) -> impl Iterator<Item = Entity<'a>> + 'a {
        let mut stack = self.direct_children(entity).collect::<Vec<_>>();
        iter::from_fn(move || -> Option<Entity<'a>> {
            let Some(entity) = stack.pop() else {
                return None;
            };

            for entity in self.direct_children(entity.id()) {
                stack.push(entity);
            }

            Some(entity)
        })
    }
}

impl<'a> Entity<'a> {
    pub fn direct_children(&'a self) -> impl Iterator<Item = Entity<'a>> {
        self.db().direct_children(self.id())
    }

    pub fn all_children(&'a self) -> impl Iterator<Item = Entity<'a>> + 'a {
        self.db().all_children(self.id())
    }

    pub fn parent(&'a self) -> Option<Entity<'a>> {
        self.component::<BelongsTo>()
            .map(|BelongsTo(parent)| self.db().entity(parent))
    }

    pub fn parents(&'a self) -> impl Iterator<Item = Entity<'a>> + 'a {
        let parent = self
            .component::<BelongsTo>()
            .map(|BelongsTo(parent)| self.db().entity(parent));

        iter::successors(parent, |x| {
            // For some reasons the lifetimes don't work out when we just call
            // `x.parent()` here
            x.component::<BelongsTo>()
                .map(|BelongsTo(parent)| self.db().entity(parent))
        })
    }

    #[tracing::instrument(level = "debug")]
    pub fn destroy_recursive(&'a self) {
        for entity in iter::once(*self).chain(self.all_children()) {
            entity.destroy()
        }
    }
}

impl<'a> NewEntity<'a> {
    pub fn direct_children(&'a self) -> impl Iterator<Item = Entity<'a>> + 'a {
        iter::empty()
    }

    pub fn all_children(&'a self) -> impl Iterator<Item = Entity<'a>> + 'a {
        iter::empty()
    }

    pub fn parent(&'a self) -> Option<Entity<'a>> {
        None
    }

    pub fn parents(&'a self) -> impl Iterator<Item = Entity<'a>> + 'a {
        iter::empty()
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

impl<'a> Entity<'a> {
    pub fn id(&self) -> EntityId {
        (self.1).0
    }

    pub fn db(&'a self) -> &'a Ecs {
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

impl<'a> Entity<'a> {
    pub fn attach<T: Component>(self, component: T) -> Self {
        self.try_attach::<T>(component).unwrap()
    }

    pub fn detach<T: Component>(self) -> Self {
        self.try_detach::<T>().unwrap()
    }

    pub fn destroy(self) {
        self.try_destroy().unwrap();
    }

    #[tracing::instrument(name = "attach", level = "debug", skip_all)]
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

    #[tracing::instrument(name = "detach", level = "debug")]
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

    #[tracing::instrument(name = "destroy", level = "debug")]
    pub fn try_destroy(self) -> Result<(), Error> {
        self.0
            .conn
            .execute("delete from components where entity = ?1", [self.id()])?;
        debug!(entity = self.id(), "destroyed");
        Ok(())
    }
}

impl<'a> NewEntity<'a> {
    pub fn or_none(self) -> Option<Self> {
        None
    }
}

impl<'a> Entity<'a> {
    pub fn or_none(self) -> Option<Self> {
        Some(self)
    }
}

impl<'a> NewEntity<'a> {
    pub fn attach<T: Component + ComponentWrite<T>>(
        self,
        component: T,
    ) -> GenericEntity<'a, WithEntityId> {
        self.try_attach::<T>(component).unwrap()
    }

    pub fn detach<T: Component>(&mut self) -> &mut Self {
        self
    }

    #[tracing::instrument(name = "attach", level = "debug", skip_all)]
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

    #[tracing::instrument(name = "detach", level = "debug", skip_all)]
    pub fn try_detach<T: Component>(&mut self) -> Result<&mut Self, Error> {
        Ok(self)
    }
}

impl<'a> std::fmt::Debug for NewEntity<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Entity").field(&"nil").finish()
    }
}

impl<'a> std::fmt::Debug for Entity<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Entity").field(&(self.1).0).finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::{self as ecsdb, Ecs};
    use crate::{BelongsTo, Component}; // #[derive(Component)] derives `impl ecsdb::Component for ...`

    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct MarkerComponent;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Component)]
    struct ComponentWithData(u64);

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct A;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Component)]
    struct B;

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
    fn belongs_to() {
        let db = Ecs::open_in_memory().unwrap();

        let parent = db.new_entity().attach(A);
        let child1 = db.new_entity().attach(A).attach(BelongsTo(parent.id()));
        let child2 = db.new_entity().attach(A).attach(BelongsTo(child1.id()));

        assert_eq!(
            parent.direct_children().map(|e| e.id()).collect::<Vec<_>>(),
            vec![child1.id()]
        );

        assert_eq!(
            parent.all_children().map(|e| e.id()).collect::<Vec<_>>(),
            vec![child1.id(), child2.id()]
        );

        assert_eq!(
            child1.all_children().map(|e| e.id()).collect::<Vec<_>>(),
            vec![child2.id()]
        );

        assert!(child2.all_children().next().is_none());
    }

    #[test]
    fn parent() {
        let db = Ecs::open_in_memory().unwrap();

        let parent = db.new_entity().attach(A);
        let child1 = db.new_entity().attach(A).attach(BelongsTo(parent.id()));
        let child2 = db.new_entity().attach(A).attach(BelongsTo(child1.id()));

        assert!(parent.parent().is_none());
        assert_eq!(child1.parent().map(|e| e.id()), Some(parent.id()));
        assert_eq!(child2.parent().map(|e| e.id()), Some(child1.id()));

        assert_eq!(
            child2.parents().map(|e| e.id()).collect::<Vec<_>>(),
            vec![child1.id(), parent.id()]
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

#[cfg(test)]
mod system_tests {
    use super::query::*;
    use super::*;
    use crate as ecsdb;

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct A;

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct B;

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct Seen;

    #[test]
    fn run() {
        let mut db = Ecs::open_in_memory().unwrap();
        fn system(query: Query<(A, B)>) {
            for entity in query.try_iter().unwrap() {
                entity.attach(Seen);
            }
        }

        db.register(system);

        let a_and_b = db.new_entity().attach(A).attach(B);
        let a = db.new_entity().attach(A);

        db.tick();
        assert!(a_and_b.component::<Seen>().is_some());
        assert!(a.component::<Seen>().is_none());
    }
}
