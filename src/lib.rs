pub mod component;
pub use component::{Component, ComponentRead, ComponentWrite};

pub mod entity;
pub use entity::{Entity, NewEntity};

pub mod extension;
pub use extension::Extension;

pub mod hierarchy;
pub use hierarchy::*;

pub mod query;

pub mod resource;
pub use resource::*;

pub mod system;
use ::rusqlite::{params, OptionalExtension};
pub use system::*;

use std::path::Path;

use query::DataFilter;
use tracing::{debug, instrument};

pub type EntityId = i64;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Database Error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    ComponentStorage(#[from] component::StorageError),
}

pub struct Ecs {
    conn: rusqlite::Connection,
    systems: Vec<Box<dyn system::System>>,
    extensions: anymap::Map<dyn anymap::any::Any + Send>,
}

impl Ecs {
    pub fn open_in_memory() -> Result<Self, Error> {
        Self::from_rusqlite(rusqlite::Connection::open_in_memory()?)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::from_rusqlite(rusqlite::Connection::open(path)?)
    }

    pub fn from_rusqlite(mut conn: rusqlite::Connection) -> Result<Self, Error> {
        conn.pragma_update(None, "journal_mode", "wal")?;
        conn.execute_batch(include_str!("schema.sql"))?;
        conn.set_transaction_behavior(::rusqlite::TransactionBehavior::Immediate);
        Ok(Self {
            conn,
            systems: Default::default(),
            extensions: anymap::Map::new(),
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

impl Ecs {
    pub fn enable_change_tracking(&mut self) -> Result<(), Error> {
        self.conn
            .execute_batch(include_str!("change_tracking_enable.sql"))?;
        Ok(())
    }

    pub fn disable_change_tracking(&mut self) -> Result<(), Error> {
        self.conn
            .execute_batch(include_str!("change_tracking_disable.sql"))?;
        Ok(())
    }
}

impl Ecs {
    pub fn new_entity<'a>(&'a self) -> NewEntity<'a> {
        Entity::without_id(self)
    }

    pub fn entity<'a>(&'a self, eid: EntityId) -> Entity<'a> {
        Entity::with_id(self, eid)
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
        query.try_into_iter()
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
        query.try_into_iter()
    }
}

impl Ecs {
    /// Returns the highest `last_modified` from all components of `entity`.
    /// Returns `chrono::DateTime::MIN_UTC` if `entity` has no components
    fn try_last_modified(
        &self,
        entity: EntityId,
    ) -> Result<chrono::DateTime<chrono::Utc>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "select max(last_modified) as last_modified from components where entity = ?",
        )?;

        let last_modified = stmt
            .query_map(params![&entity], |row| {
                row.get::<_, Option<String>>("last_modified")
            })?
            .flat_map(|dt| {
                dt.unwrap_or_else(|e| panic!("max(last_modified) on {entity} TEXT error={e}"))
            })
            .next();

        let last_modified = if let Some(last_modified) = last_modified {
            chrono::DateTime::parse_from_rfc3339(&last_modified)
                .expect("Valid chrono::DateTime")
                .to_utc()
        } else {
            chrono::DateTime::<chrono::Utc>::MIN_UTC
        };

        Ok(last_modified)
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

        Ok(rows
            .into_iter()
            .scan(self, |ecs, eid| Some(Entity::with_id(&ecs, eid))))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    Create { entity: EntityId },
    Attach { entity: EntityId, component: String },
    Detach { entity: EntityId, component: String },
    Destroy { entity: EntityId },
}

impl Ecs {
    pub fn latest_change_id(&self) -> Result<Option<i64>, Error> {
        let seq: Option<i64> = self.conn.query_row_and_then(
            "select max(sequence) from changes",
            params![],
            |row| row.get(0),
        )?;

        Ok(seq)
    }

    pub fn clear_changes_up_to(&self, up_to: i64) -> Result<(), Error> {
        self.conn
            .execute("delete from changes where sequence < ?1", params![up_to])?;
        Ok(())
    }

    pub fn changes(&self) -> Result<Vec<Change>, Error> {
        let mut stmt = self
            .conn
            .prepare_cached("select * from changes order by sequence asc")?;

        let changes = stmt
            .query_map(params![], |row| {
                let entity = row.get("entity")?;
                let change: String = row.get("change")?;

                match change.as_str() {
                    "create" => Ok(Change::Create { entity }),
                    "attach" => {
                        let component = row.get("component")?;
                        Ok(Change::Attach { entity, component })
                    }
                    "detach" => {
                        let component = row.get("component")?;
                        Ok(Change::Detach { entity, component })
                    }
                    "destroy" => Ok(Change::Destroy { entity }),
                    other => {
                        panic!("Invalid 'changes.change' {other:?}");
                    }
                }
            })?
            .collect::<Result<_, _>>()?;

        Ok(changes)
    }

    pub fn clear_changes(&self) -> Result<(), Error> {
        self.conn.execute("delete from changes", params![])?;
        Ok(())
    }
}

pub mod rusqlite {
    pub use rusqlite::*;
}

impl Ecs {
    pub fn raw_sql<'a>(&'a self) -> &'a rusqlite::Connection {
        &self.conn
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
                Self::Table => "components",
                Self::Entity => "entity",
                Self::Component => "component",
                Self::Data => "data",
            };
            write!(s, "{v}").unwrap()
        }
    }

    #[allow(unused)]
    pub enum Changes {
        Table,
        Sequence,
        Entity,
        Component,
        Change,
    }

    impl sea_query::Iden for Changes {
        fn unquoted(&self, s: &mut dyn std::fmt::Write) {
            let v = match self {
                Self::Table => "changes",
                Self::Sequence => "sequence",
                Self::Entity => "entity",
                Self::Component => "component",
                Self::Change => "change",
            };
            write!(s, "{v}").unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    // #[derive(Component)] derives `impl ecsdb::Component for ...`
    use crate::{self as ecsdb, Change, Ecs};
    use crate::{BelongsTo, Component};

    use anyhow::anyhow;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct MarkerComponent;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Component)]
    struct ComponentWithData(u64);

    #[derive(Debug, Serialize, Deserialize, Component)]
    struct A;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Component)]
    struct B;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Component)]
    struct C;

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

    #[test]
    fn attaching_same_component_in_bundle_overwrites() {
        let db = super::Ecs::open_in_memory().unwrap();

        let entity = db
            .new_entity()
            .attach((ComponentWithData(42), ComponentWithData(23)));
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
    fn or() {
        let db = Ecs::open_in_memory().unwrap();
        let a = db.new_entity().attach(A).id();
        let b = db.new_entity().attach(B).id();
        let c = db.new_entity().attach(C).id();

        assert_eq!(
            db.query::<Or<(A, B, C)>>()
                .map(|e| e.id())
                .collect::<Vec<_>>(),
            vec![a, b, c]
        );
        assert_eq!(
            db.query::<Or<(A, B)>>().map(|e| e.id()).collect::<Vec<_>>(),
            vec![a, b]
        );
        assert_eq!(
            db.query::<Or<(A,)>>().map(|e| e.id()).collect::<Vec<_>>(),
            vec![a]
        );
        assert_eq!(
            db.query::<Or<(B,)>>().map(|e| e.id()).collect::<Vec<_>>(),
            vec![b]
        );
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

    #[test]
    fn has_many() {
        let db = Ecs::open_in_memory().unwrap();
        let a = db.new_entity().attach(A);
        assert!(a.has::<A>());
        assert!(!a.has::<B>());

        assert_eq!(a.has::<(A,)>(), true);
        assert_eq!(a.has::<(A, B)>(), false);
        assert_eq!(a.has::<(A, B, A)>(), false);

        let ab = db.new_entity().attach(A).attach(B);
        assert_eq!(ab.has::<(A, B)>(), true);
        assert_eq!(ab.has::<(A, A)>(), true);
        assert_eq!(ab.has::<(A, B, A)>(), true);
    }

    #[test]
    fn from_component_composite() -> Result<(), anyhow::Error> {
        #[derive(Serialize, Deserialize, Component)]
        struct A;
        #[derive(Serialize, Deserialize, Component)]
        struct B;

        let db = super::Ecs::open_in_memory()?;
        let _e = db.new_entity().attach((A, B));

        // let ab = e.component::<(A, B)>();
        // assert!(ab.is_some());

        Ok(())
    }

    #[test]
    fn entity_matches() {
        #[derive(Serialize, Deserialize, Component)]
        struct A;
        #[derive(Serialize, Deserialize, Component)]
        struct B;

        let db = super::Ecs::open_in_memory().unwrap();
        let e = db.new_entity().attach(A);
        let e2 = db.new_entity().attach((A, B));

        assert!(e.matches::<A>());
        assert!(!e.matches::<B>());
        assert!(!e.matches::<(A, B)>());

        assert!(e2.matches::<A>());
        assert!(e2.matches::<B>());
        assert!(e2.matches::<(A, B)>());
    }

    #[test]
    fn last_modified() {
        #[derive(Serialize, Deserialize, Component)]
        struct A;
        #[derive(Serialize, Deserialize, Component)]
        struct B;

        let db = super::Ecs::open_in_memory().unwrap();
        let e = db.new_entity().attach(A);

        assert!(e.last_modified() > chrono::Utc::now() - chrono::Duration::minutes(1));

        let old = e.last_modified();

        std::thread::sleep(std::time::Duration::from_millis(2));

        e.attach(B);
        assert!(e.last_modified() > old);
    }

    #[test]
    fn modify_component() -> Result<(), anyhow::Error> {
        let ecs = super::Ecs::open_in_memory()?;

        #[derive(Component, Debug, Default, Deserialize, Serialize, PartialEq)]
        struct Foo(Vec<u64>);

        let entity = ecs.new_entity().modify_component(|foo: &mut Foo| {
            *foo = Foo(vec![1, 2, 3]);
        });
        assert_eq!(entity.component(), Some(Foo(vec![1, 2, 3])));

        let entity = ecs
            .new_entity()
            .attach(Foo(vec![1, 2, 3]))
            .modify_component(|foo: &mut Foo| {
                foo.0.clear();
            });

        assert_eq!(entity.component(), Some(Foo(vec![])));

        Ok(())
    }

    #[test]
    fn try_modify_component() -> Result<(), anyhow::Error> {
        let ecs = super::Ecs::open_in_memory()?;

        #[derive(Component, Debug, Default, Deserialize, Serialize, PartialEq)]
        struct Foo(Vec<u64>);

        assert!(ecs
            .new_entity()
            .try_modify_component(|_foo: &mut Foo| { Err(anyhow!("error")) })
            .is_err());

        assert!(ecs
            .new_entity()
            .attach(Foo(vec![1, 2, 3]))
            .try_modify_component(|_foo: &mut Foo| { Err(anyhow!("error")) })
            .is_err());

        Ok(())
    }

    #[test]
    fn change_tracking_enable_disable() -> Result<(), anyhow::Error> {
        let mut ecs = super::Ecs::open_in_memory()?;
        ecs.enable_change_tracking()?;

        assert_eq!(ecs.changes()?, vec![]);

        ecs.new_entity().attach(A);
        assert_eq!(ecs.changes()?.len(), 2);

        ecs.disable_change_tracking()?;
        ecs.clear_changes()?;

        ecs.new_entity().attach(A);
        assert!(ecs.changes()?.is_empty());

        Ok(())
    }

    #[test]
    fn change_tracking() -> Result<(), anyhow::Error> {
        let mut ecs = super::Ecs::open_in_memory()?;
        ecs.enable_change_tracking()?;

        let mut changes = vec![];

        assert_eq!(ecs.changes()?, vec![]);

        let entity = ecs.new_entity().attach(A);

        changes.extend([
            Change::Create {
                entity: entity.id(),
            },
            Change::Attach {
                entity: entity.id(),
                component: <A as Component>::component_name().to_owned(),
            },
        ]);

        assert_eq!(ecs.changes()?, changes);

        entity.attach(B);
        changes.push(Change::Attach {
            entity: entity.id(),
            component: <B as Component>::component_name().to_owned(),
        });

        assert_eq!(ecs.changes()?, changes);

        entity.detach::<B>();
        changes.push(Change::Detach {
            entity: entity.id(),
            component: <B as Component>::component_name().to_owned(),
        });

        assert_eq!(ecs.changes()?, changes);

        entity.detach::<A>();
        changes.extend([
            Change::Detach {
                entity: entity.id(),
                component: <A as Component>::component_name().to_owned(),
            },
            Change::Destroy {
                entity: entity.id(),
            },
        ]);

        assert_eq!(ecs.changes()?, changes);

        ecs.clear_changes()?;
        assert!(ecs.changes()?.is_empty());

        Ok(())
    }

    #[test]
    fn changed_system_param() -> Result<(), anyhow::Error> {
        #[derive(Debug, Deserialize, Serialize, Component)]
        struct Seen;

        let mut ecs = super::Ecs::open_in_memory()?;
        ecs.enable_change_tracking()?;

        fn system(query: Query<Attached<B>>) {
            assert_eq!(query.iter().map(|e| e.id()).collect::<Vec<_>>(), vec![200]);

            query.iter().for_each(|e| {
                e.attach(Seen);
            });
        }

        ecs.register(system);

        ecs.entity(100).attach(A);
        ecs.entity(200).attach(B);

        ecs.tick();
        assert!(ecs.entity(200).has::<Seen>());

        Ok(())
    }
}
