use ecsdb_derive::with_infallible;
use rusqlite::{OptionalExtension, params};
use tracing::{debug, trace};

use crate::{
    Component, CreatedAt, DynComponent, Ecs, EntityId, Error, LastUpdated,
    component::{Bundle, NonEmptyBundle},
    query::{self},
};

#[derive(Debug, Copy, Clone)]
pub struct WithoutEntityId;
#[derive(Debug, Copy, Clone)]
pub struct WithEntityId(EntityId);

pub type Entity<'a> = GenericEntity<'a, WithEntityId>;
pub type NewEntity<'a> = GenericEntity<'a, WithoutEntityId>;

#[derive(Copy, Clone)]
pub struct GenericEntity<'a, S>(&'a Ecs, S);

impl<'a, T> GenericEntity<'a, T> {
    pub(crate) fn without_id(ecs: &'a Ecs) -> NewEntity<'a> {
        GenericEntity(ecs, WithoutEntityId)
    }

    pub(crate) fn with_id(ecs: &'a Ecs, eid: EntityId) -> Entity<'a> {
        GenericEntity(ecs, WithEntityId(eid))
    }

    pub fn db(&'a self) -> &'a Ecs {
        self.0
    }

    /// Begin a new `IMMEDIATE` transaction on the entity's connection.
    ///
    /// # Warning
    ///
    /// Callers must ensure no other transaction is live on `self.0.conn` for
    /// the duration of the returned transaction.
    ///
    /// In particular, do not invoke mutating entity methods from inside a
    /// [`try_modify_component`] closure, or while iterating a query/statement
    /// that holds the connection in a transaction.
    ///
    /// In other words: The function should be pure regarding database access.
    fn immediate_tx(&self) -> Result<rusqlite::Transaction<'_>, Error> {
        Ok(rusqlite::Transaction::new_unchecked(
            &self.0.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?)
    }
}

impl<'a> Entity<'a> {
    pub fn id(self) -> EntityId {
        (self.1).0
    }
}

#[with_infallible]
impl<'a> Entity<'a> {
    #[tracing::instrument(name = "exists", level = "debug")]
    pub fn try_exists(self) -> Result<bool, Error> {
        self.0
            .conn
            .query_row(
                "select true from components where entity = ?1",
                params![self.id()],
                |_| Ok(()),
            )
            .optional()
            .map(|o| o.is_some())
            .map_err(Error::from)
    }

    #[tracing::instrument(name = "created_at", level = "debug")]
    pub fn try_created_at(self) -> Result<chrono::DateTime<chrono::Utc>, Error> {
        self.try_component()
            .map(Option::unwrap_or_default)
            .map(|CreatedAt(lu)| lu)
    }

    #[tracing::instrument(name = "last_modified", level = "debug")]
    pub fn try_last_modified(self) -> Result<chrono::DateTime<chrono::Utc>, Error> {
        self.try_component()
            .map(Option::unwrap_or_default)
            .map(|LastUpdated(lu)| lu)
    }

    #[tracing::instrument(name = "component_names", level = "debug")]
    pub fn try_component_names(self) -> Result<impl Iterator<Item = String>, Error> {
        let mut stmt = self
            .0
            .conn
            .prepare_cached("select component from components where entity = ?1")?;
        let names = stmt
            .query_map(params![self.id()], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(names.into_iter())
    }
}

#[with_infallible]
impl<'a> Entity<'a> {
    pub fn try_has<B: Bundle>(self) -> Result<bool, Error> {
        self.try_has_all_dynamic(B::COMPONENTS)
    }

    fn try_has_all_dynamic(self, component_names: &[&str]) -> Result<bool, Error> {
        let mut stmt = self
            .0
            .conn
            .prepare_cached("select true from components where entity = ?1 and component = ?2")?;
        for name in component_names {
            if !stmt.exists(params![self.id(), name])? {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

#[with_infallible]
impl<'a> Entity<'a> {
    #[tracing::instrument(name = "destroy", level = "debug")]
    pub fn try_destroy(self) -> Result<(), Error> {
        self.0
            .conn
            .execute("delete from components where entity = ?1", [self.id()])?;
        debug!(entity = self.id(), "destroyed");
        Ok(())
    }
}

#[with_infallible]
impl<'a> Entity<'a> {
    pub fn try_component<T: Component>(self) -> Result<Option<T>, Error> {
        self.try_component_within(&self.0.conn)
    }
}

#[with_infallible]
impl<'a> Entity<'a> {
    pub fn try_dyn_component(self, name: &'a str) -> Result<Option<DynComponent<'a>>, Error> {
        self.try_dyn_component_within(&self.0.conn, name)
    }
}

impl<'a> Entity<'a> {
    /// Upsert a sequence of `(component_name, data)` pairs using the given
    /// connection (which may be a [`rusqlite::Transaction`]). The component data is inserted or
    /// overwritten. Unchanged data is a no-op.
    fn attach_within<I, D>(self, conn: &rusqlite::Connection, components: I) -> Result<(), Error>
    where
        I: IntoIterator<Item = (&'a str, D)>,
        D: rusqlite::ToSql,
    {
        let mut query = conn.prepare_cached(
            r#"
            insert into components (entity, component, data)
            values (?2, ?3, ?1)
            on conflict (entity, component) do update
            set data = excluded.data where data is not excluded.data
            "#,
        )?;

        for (component, data) in components {
            trace!(params = ?(self.id(), component));

            let attached_rows = query.execute(params![data, self.id(), component])?;
            if attached_rows > 0 {
                debug!(entity = self.id(), component, "attached");
            } else {
                debug!(entity = self.id(), component, "no-op");
            }
        }

        Ok(())
    }

    /// Delete a sequence of components (by name) from the entity using the
    /// given connection (which may be a [`rusqlite::Transaction`]).
    fn detach_within<I>(self, conn: &rusqlite::Connection, components: I) -> Result<(), Error>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut query =
            conn.prepare_cached("delete from components where entity = ?1 and component = ?2")?;

        for component in components {
            let deleted_rows = query.execute(params![self.id(), component])?;
            if deleted_rows > 0 {
                debug!(entity = self.id(), component, "detached");
            } else {
                debug!(entity = self.id(), component, "no-op");
            }
        }

        Ok(())
    }

    /// Read a single typed component using the given connection (which may be
    /// a [`rusqlite::Transaction`]).
    fn try_component_within<T: Component>(
        self,
        conn: &rusqlite::Connection,
    ) -> Result<Option<T>, Error> {
        let name = T::component_name();
        let mut query = conn
            .prepare_cached("select data from components where entity = ?1 and component = ?2")?;

        query
            .query_and_then(params![self.id(), name], |row| {
                let data = row.get_ref("data")?;
                Ok(T::from_rusqlite(&rusqlite::types::ToSqlOutput::Borrowed(
                    data,
                ))?)
            })?
            .next()
            .transpose()
    }

    /// Read a single component by name using the given connection (which may be
    /// a [`rusqlite::Transaction`], since it derefs to [`rusqlite::Connection`]).
    fn try_dyn_component_within(
        self,
        conn: &rusqlite::Connection,
        name: &'a str,
    ) -> Result<Option<DynComponent<'a>>, Error> {
        let mut query = conn
            .prepare_cached("select data from components where entity = ?1 and component = ?2")?;

        query
            .query_and_then(params![self.id(), name], |row| {
                let data = row.get("data")?;
                Ok(DynComponent(
                    name,
                    rusqlite::types::ToSqlOutput::Owned(data),
                ))
            })?
            .next()
            .transpose()
    }
}

#[with_infallible]
impl<'a> Entity<'a> {
    /// Attach a [`DynComponent`] by name, inserting it if the entity does not
    /// already have that component and overwriting its data otherwise.
    #[tracing::instrument(name = "dyn_attach", level = "debug", skip_all)]
    pub fn try_dyn_attach(self, component: DynComponent<'a>) -> Result<Self, Error> {
        let tx = self.immediate_tx()?;
        self.attach_within(&tx, [(component.0, component.1)])?;
        tx.commit()?;
        Ok(self)
    }
}

#[with_infallible]
impl<'a> Entity<'a> {
    pub fn try_detach_named(self, component: &'a str) -> Result<Self, Error> {
        let tx = self.immediate_tx()?;
        self.detach_within(&tx, [component])?;
        tx.commit()?;
        Ok(self)
    }
}

impl<'a> Entity<'a> {
    pub fn modify_component<C: Component + Default>(self, f: impl FnOnce(&mut C)) -> Self {
        self.try_modify_component(|c| {
            f(c);
            Ok(())
        })
        .unwrap()
    }

    /// Read-modify-write a single component atomically.
    ///
    /// The read, the modification closure, and the write all happen inside a
    /// single `IMMEDIATE` transaction, so a concurrent writer cannot change
    /// the component between the read and the write.
    ///
    /// # Warning
    ///
    /// The closure `f` runs **inside the open transaction**. It must not
    /// perform any other database mutation on the same [`Ecs`] (e.g.
    /// `entity.attach(..)`, `entity.modify_component(..)`, `db.new_entity()`),
    /// as that would try to open a second transaction on a connection that is
    /// already in one and fail at runtime. Keep `f` to in-memory mutation of
    /// the passed component only.
    pub fn try_modify_component<C: Component + Default>(
        self,
        f: impl FnOnce(&mut C) -> Result<(), anyhow::Error>,
    ) -> Result<Self, ModifyComponentError> {
        let tx = self.immediate_tx()?;

        let mut component = self.try_component_within(&tx)?.unwrap_or_default();
        f(&mut component).map_err(ModifyComponentError::Fn)?;

        let data = C::to_rusqlite(&component).map_err(Error::from)?;
        self.attach_within(&tx, [(C::component_name(), data)])?;

        tx.commit().map_err(Error::from)?;
        Ok(self)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ModifyComponentError {
    #[error(transparent)]
    Ecs(#[from] Error),
    #[error("Error in modify-fun: {0}")]
    Fn(anyhow::Error),
}

#[with_infallible]
impl<'a> Entity<'a> {
    pub fn try_matches<D: query::QueryFilter>(self) -> Result<bool, Error> {
        let q = query::Query::<(), D, EntityId>::with_filter(self.db(), self.id());
        Ok(q.try_iter()?.next().is_some())
    }
}

#[with_infallible]
impl<'a> Entity<'a> {
    #[tracing::instrument(name = "attach", level = "debug", skip_all)]
    pub fn try_attach<B: Bundle>(self, component: B) -> Result<Self, Error> {
        let components = B::to_rusqlite(&component)?
            .into_iter()
            .filter_map(|(component, data)| match data {
                Some(data) => Some((component, data)),
                None => {
                    debug!(component, "skipping None");
                    None
                }
            });

        let tx = self.immediate_tx()?;
        self.attach_within(&tx, components)?;
        tx.commit()?;

        Ok(self)
    }

    #[tracing::instrument(name = "detach", level = "debug")]
    pub fn try_detach<B: Bundle>(self) -> Result<Self, Error> {
        let tx = self.immediate_tx()?;
        self.detach_within(&tx, B::COMPONENTS.iter().copied())?;
        tx.commit()?;

        Ok(self)
    }
}

#[with_infallible]
impl<'a> Entity<'a> {
    #[tracing::instrument(name = "detach_all", level = "debug")]
    pub fn try_detach_all(self) -> Result<Self, Error> {
        self
            .0
            .conn
            .execute("delete from components where entity = ?1 and component not in (select component from system_components)", params![self.id()])?;

        Ok(self)
    }
}

impl<'a> Entity<'a> {
    pub fn or_none(self) -> Option<Self> {
        self.exists().then_some(self)
    }
}

/// Connection/transaction-scoped helpers shared by the public `NewEntity` API.
/// These take an explicit connection (typically a transaction) so the id
/// allocation and all component inserts happen atomically.
impl<'a> NewEntity<'a> {
    /// Insert a sequence of `(component_name, data)` pairs for a brand new
    /// entity using the given connection (which may be a
    /// [`rusqlite::Transaction`], since it derefs to [`rusqlite::Connection`]),
    /// allocating the entity id on the first inserted row and threading it into
    /// the remaining rows. Returns the allocated [`EntityId`].
    ///
    /// Panics if the iterator yields no rows, as a new entity must have at
    /// least one component to exist.
    fn attach_new_within<I, D>(
        self,
        conn: &rusqlite::Connection,
        components: I,
    ) -> Result<EntityId, Error>
    where
        I: IntoIterator<Item = (&'a str, D)>,
        D: rusqlite::ToSql,
    {
        let mut stmt = conn.prepare_cached(
            r#"
            insert into components (entity, component, data)
            values ((select coalesce(?1, max(entity)+1, 100) from components), ?2, ?3)
            on conflict (entity, component) do update set data = excluded.data
            returning entity
            "#,
        )?;

        let mut eid = None;
        for (component, data) in components {
            trace!(params = ?(eid, component));

            eid = Some(stmt.query_row(params![eid, component, data], |row| {
                row.get::<_, EntityId>("entity")
            })?);

            debug!(entity = eid.unwrap(), component, "attached");
        }

        let Some(eid) = eid else {
            panic!("attach_new_within was called with zero rows. That shouldn't happen.")
        };

        Ok(eid)
    }
}

#[with_infallible]
impl<'a> NewEntity<'a> {
    #[tracing::instrument(name = "attach", level = "debug", skip_all)]
    pub fn try_attach<B: NonEmptyBundle>(
        self,
        bundle: B,
    ) -> Result<GenericEntity<'a, WithEntityId>, Error> {
        let components = B::to_rusqlite(&bundle)?
            .into_iter()
            .filter_map(|(component, data)| match data {
                Some(data) => Some((component, data)),
                None => {
                    debug!(component, "skipping None");
                    None
                }
            });

        let tx = self.immediate_tx()?;
        let eid = self.attach_new_within(&tx, components)?;
        tx.commit()?;

        Ok(GenericEntity(self.0, WithEntityId(eid)))
    }

    /// Create a new entity from a single [`DynComponent`], returning the new
    /// [`Entity`]. This is the dynamic (string-name) counterpart to
    /// [`try_attach`](Self::try_attach).
    #[tracing::instrument(name = "dyn_attach", level = "debug", skip_all)]
    pub fn try_dyn_attach(
        self,
        component: DynComponent<'a>,
    ) -> Result<GenericEntity<'a, WithEntityId>, Error> {
        let tx = self.immediate_tx()?;
        let eid = self.attach_new_within(&tx, [(component.0, component.1)])?;
        tx.commit()?;

        Ok(GenericEntity(self.0, WithEntityId(eid)))
    }

    #[tracing::instrument(name = "detach", level = "debug", skip_all)]
    pub fn try_detach<B: Bundle>(&mut self) -> Result<&mut Self, Error> {
        Ok(self)
    }

    #[tracing::instrument(name = "component_names", level = "debug")]
    pub fn try_component_names(self) -> Result<impl Iterator<Item = String>, Error> {
        Ok(std::iter::empty())
    }
}

impl<'a> NewEntity<'a> {
    pub fn modify_component<C: Component + Default>(self, f: impl FnOnce(&mut C)) -> Entity<'a> {
        self.try_modify_component(|c| {
            f(c);
            Ok(())
        })
        .unwrap()
    }

    /// A new entity has no existing component to read, so this starts from
    /// `C::default()` and attaches. No read-modify-write race exists here.
    pub fn try_modify_component<C: Component + Default>(
        self,
        f: impl FnOnce(&mut C) -> Result<(), anyhow::Error>,
    ) -> Result<Entity<'a>, ModifyComponentError> {
        let mut component = C::default();
        f(&mut component).map_err(ModifyComponentError::Fn)?;
        Ok(self.try_attach(component)?)
    }
}

impl<'a> std::fmt::Display for NewEntity<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Entity").field(&"nil").finish()
    }
}

impl<'a> std::fmt::Display for Entity<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Entity").field(&(self.1).0).finish()
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
