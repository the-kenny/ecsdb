use rusqlite::{params, OptionalExtension};
use tracing::{debug, trace};

use crate::{
    component::Bundle,
    query::{self, FilterValueWrapper},
    Component, CreatedAt, DynComponent, Ecs, EntityId, Error, LastUpdated,
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
}

impl<'a> Entity<'a> {
    pub fn id(&self) -> EntityId {
        (self.1).0
    }

    pub fn exists(&self) -> bool {
        self.try_exists().expect("Entity::try_exists")
    }

    #[tracing::instrument(name = "exists", level = "debug")]
    pub fn try_exists(&self) -> Result<bool, Error> {
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

    pub fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.try_created_at().expect("Non-Error")
    }

    #[tracing::instrument(name = "created_at", level = "debug")]
    pub fn try_created_at(&self) -> Result<chrono::DateTime<chrono::Utc>, Error> {
        self.try_component()
            .map(Option::unwrap_or_default)
            .map(|CreatedAt(lu)| lu)
    }

    pub fn last_modified(&self) -> chrono::DateTime<chrono::Utc> {
        self.try_last_modified().expect("Non-Error")
    }

    #[tracing::instrument(name = "last_modified", level = "debug")]
    pub fn try_last_modified(&self) -> Result<chrono::DateTime<chrono::Utc>, Error> {
        self.try_component()
            .map(Option::unwrap_or_default)
            .map(|LastUpdated(lu)| lu)
    }

    pub fn component_names(&self) -> impl Iterator<Item = String> {
        self.try_component_names().unwrap()
    }

    #[tracing::instrument(name = "component_names", level = "debug")]
    pub fn try_component_names(&self) -> Result<impl Iterator<Item = String>, Error> {
        let mut stmt = self
            .0
            .conn
            .prepare("select component from components where entity = ?1")?;
        let names = stmt
            .query_map(params![self.id()], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(names.into_iter())
    }

    pub fn has<B: Bundle>(&self) -> bool {
        self.try_has::<B>().unwrap()
    }

    pub fn try_has<B: Bundle>(&self) -> Result<bool, Error> {
        self.has_all_dynamic(B::COMPONENTS)
    }

    fn has_all_dynamic(&self, component_names: &[&str]) -> Result<bool, Error> {
        let mut stmt = self
            .0
            .conn
            .prepare("select true from components where entity = ?1 and component = ?2")?;
        for name in component_names {
            if !stmt.exists(params![self.id(), name])? {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

impl<'a> Entity<'a> {
    pub fn destroy(self) {
        self.try_destroy().unwrap();
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

impl<'a> Entity<'a> {
    pub fn component<T: Component>(&self) -> Option<T> {
        match self.try_component::<T>() {
            Ok(c) => c,
            Err(e) => panic!("Failed to get Component {}: {e}", T::NAME),
        }
    }

    pub fn try_component<T: Component>(&self) -> Result<Option<T>, Error> {
        let name = T::component_name();
        let mut query = self
            .0
            .conn
            .prepare_cached("select data from components where entity = ?1 and component = ?2")?;

        let row = query
            .query_and_then(params![self.id(), name], |row| {
                let data = row.get_ref("data")?;
                Ok(T::from_rusqlite(&rusqlite::types::ToSqlOutput::Borrowed(
                    data,
                ))?)
            })?
            .next()
            .transpose();

        row
    }
}

impl<'a> Entity<'a> {
    pub fn dyn_component(&self, name: &'a str) -> Option<DynComponent<'a>> {
        self.try_dyn_component(name).unwrap()
    }

    pub fn try_dyn_component(&self, name: &'a str) -> Result<Option<DynComponent<'a>>, Error> {
        let mut query = self
            .0
            .conn
            .prepare_cached("select data from components where entity = ?1 and component = ?2")?;

        let row = query
            .query_and_then(params![self.id(), name], |row| {
                let data = row.get("data")?;
                Ok(DynComponent(
                    name,
                    rusqlite::types::ToSqlOutput::Owned(data),
                ))
            })?
            .next()
            .transpose();

        row
    }
}

impl<'a> Entity<'a> {
    pub fn modify_component<C: Component + Default>(&self, f: impl FnOnce(&mut C)) -> Self {
        self.try_modify_component(|c| {
            f(c);
            Ok(())
        })
        .unwrap()
    }

    // TODO: Race Condition; needs refactoring to make Entity generic over
    // `rusqlite::Connection` and `rusqlite::Transaction`
    pub fn try_modify_component<C: Component + Default>(
        &self,
        f: impl FnOnce(&mut C) -> Result<(), anyhow::Error>,
    ) -> Result<Self, ModifyComponentError> {
        let mut component = self.try_component()?.unwrap_or_default();
        f(&mut component).map_err(ModifyComponentError::Fn)?;
        Ok(self.try_attach(component)?)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ModifyComponentError {
    #[error(transparent)]
    Ecs(#[from] Error),
    #[error("Error in modify-fun: {0}")]
    Fn(anyhow::Error),
}

impl<'a> Entity<'a> {
    pub fn try_matches<D: query::QueryFilter + Default>(&self) -> Result<bool, Error> {
        let q = query::Query::<(), (FilterValueWrapper<EntityId>, _)>::new(
            self.db(),
            (self.id().into(), D::default()),
        );
        Ok(q.try_iter()?.next().is_some())
    }

    pub fn matches<D: query::QueryFilter + Default>(&self) -> bool {
        self.try_matches::<D>().unwrap()
    }

    // pub fn try_matches_filtered<F: query::QueryFilter>(&self, filter: F) -> Result<bool, Error> {
    //     let q = query::Query::<(), F>::new(self.db(), filter);
    //     Ok(q.try_iter()?.next().is_some())
    // }

    // pub fn matches_filtered<F: query::QueryFilter>(&self, filter: F) -> bool {
    //     self.try_matches_filtered::<F>(filter).unwrap()
    // }
}

impl<'a> Entity<'a> {
    pub fn attach<B: Bundle>(self, component: B) -> Self {
        self.try_attach::<B>(component).unwrap()
    }

    pub fn detach<B: Bundle>(self) -> Self {
        self.try_detach::<B>().unwrap()
    }

    #[tracing::instrument(name = "attach", level = "debug", skip_all)]
    pub fn try_attach<B: Bundle>(self, component: B) -> Result<Self, Error> {
        let components = B::to_rusqlite(&component)?;

        let mut stmt = self.0.conn.prepare(
            r#"
            insert into components (entity, component, data)
            values (?1, ?2, ?3)
            on conflict (entity, component) do update
            set data = excluded.data where data is not excluded.data;
            "#,
        )?;

        for (component, data) in components {
            trace!(params = ?(self.id(), component, &data));

            if let Some(data) = data {
                let attached_rows = stmt.execute(params![self.id(), component, data])?;
                if attached_rows > 0 {
                    debug!(entity = self.id(), component, "attached");
                } else {
                    debug!(entity = self.id(), component, "no-op")
                }
            } else {
                debug!(component, ?data, "skipping None");
            }
        }

        Ok(self)
    }

    #[tracing::instrument(name = "detach", level = "debug")]
    pub fn try_detach<B: Bundle>(self) -> Result<Self, Error> {
        let mut stmt = self
            .0
            .conn
            .prepare("delete from components where entity = ?1 and component = ?2")?;

        for component in B::COMPONENTS {
            let deleted_rows = stmt.execute(params![self.id(), component])?;
            if deleted_rows > 0 {
                debug!(entity = self.id(), component, "detached");
            } else {
                debug!(entity = self.id(), component, "no-op")
            }
        }

        Ok(self)
    }
}

impl<'a> Entity<'a> {
    pub fn or_none(self) -> Option<Self> {
        self.exists().then_some(self)
    }
}

impl<'a> NewEntity<'a> {
    pub fn attach<B: Bundle>(self, component: B) -> GenericEntity<'a, WithEntityId> {
        self.try_attach::<B>(component).unwrap()
    }

    pub fn detach<B: Bundle>(&mut self) -> &mut Self {
        self
    }

    pub fn component_names(&self) -> impl Iterator<Item = String> {
        std::iter::empty()
    }

    #[tracing::instrument(name = "attach", level = "debug", skip_all)]
    pub fn try_attach<B: Bundle>(
        self,
        bundle: B,
    ) -> Result<GenericEntity<'a, WithEntityId>, Error> {
        let data = B::to_rusqlite(&bundle)?;
        assert!(!data.is_empty());

        let mut stmt = self.0.conn.prepare(
            r#"
            insert into components (entity, component, data)
            values ((select coalesce(?1, max(entity)+1, 100) from components), ?2, ?3)
            on conflict (entity, component) do update set data = excluded.data
            returning entity
            "#,
        )?;

        let mut eid = None;
        for (component, data) in data {
            trace!(params = ?(eid, component, &data));

            if let Some(data) = data {
                eid = Some(stmt.query_row(params![eid, component, data], |row| {
                    row.get::<_, EntityId>("entity")
                })?);

                debug!(entity = eid.unwrap(), component, "attached");
            } else {
                debug!(component, ?data, "skipping None");
            }
        }

        let Some(eid) = eid else {
            panic!("Bundle::to_rusqlite returned zero rows. That shouldn't happen.")
        };

        let entity = GenericEntity(self.0, WithEntityId(eid));

        Ok(entity)
    }

    #[tracing::instrument(name = "detach", level = "debug", skip_all)]
    pub fn try_detach<B: Bundle>(&mut self) -> Result<&mut Self, Error> {
        Ok(self)
    }

    #[tracing::instrument(name = "component_names", level = "debug")]
    pub fn try_component_names(&self) -> Result<impl Iterator<Item = String>, Error> {
        Ok(std::iter::empty())
    }
}

impl<'a> NewEntity<'a> {
    pub fn modify_component<C: Component + Default>(&self, f: impl FnOnce(&mut C)) -> Entity<'a> {
        self.try_modify_component(|c| {
            f(c);
            Ok(())
        })
        .unwrap()
    }

    // TODO: Race Condition; needs refactoring to make Entity generic over
    // `rusqlite::Connection` and `rusqlite::Transaction`
    pub fn try_modify_component<C: Component + Default>(
        &self,
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
