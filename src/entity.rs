use std::iter;

use rusqlite::params;
use tracing::debug;

use crate::{BelongsTo, Component, ComponentWrite, Ecs, EntityId, Error};

#[derive(Debug, Copy, Clone)]
pub struct WithoutEntityId;
#[derive(Debug, Copy, Clone)]
pub struct WithEntityId(EntityId);

pub type Entity<'a> = GenericEntity<'a, WithEntityId>;
pub type NewEntity<'a> = GenericEntity<'a, WithoutEntityId>;

#[derive(Copy, Clone)]
pub struct GenericEntity<'a, S>(&'a Ecs, S);

impl<'a, T> GenericEntity<'a, T> {
    pub(crate) fn without_id(ecs: &'a Ecs) -> NewEntity {
        GenericEntity(ecs, WithoutEntityId)
    }

    pub(crate) fn with_id(ecs: &'a Ecs, eid: EntityId) -> Entity {
        GenericEntity(ecs, WithEntityId(eid))
    }
}

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
