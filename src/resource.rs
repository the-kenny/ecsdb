use std::ops::{Deref, DerefMut};

pub use ecsdb_derive::Resource;

use rusqlite::params;
use tracing::debug;

use crate::{Component, Ecs, Error};

pub trait Resource: Component {
    fn resource_name() -> &'static str {
        <Self as Component>::component_name()
    }
}

impl Ecs {
    pub fn resource<R: Resource>(&self) -> Option<R> {
        self.try_resource::<R>().unwrap()
    }

    pub fn try_resource<R: Resource>(&self) -> Result<Option<R>, Error> {
        let name = R::resource_name();
        let mut query = self
            .conn
            .prepare("select data from resources where name = ?1")?;
        let row = query
            .query_and_then(params![name], |row| {
                row.get::<_, rusqlite::types::Value>("data")
            })?
            .next();

        match row {
            None => Ok(None),
            Some(Ok(data)) => {
                let component = R::from_rusqlite(data)?;
                Ok(Some(component))
            }
            _other => panic!(),
        }
    }

    pub fn resource_mut<'a, R: Resource + Default>(&'a mut self) -> impl DerefMut<Target = R> + 'a {
        self.try_resource_mut().unwrap()
    }

    pub fn try_resource_mut<'a, R: Resource + Default>(
        &'a mut self,
    ) -> Result<impl DerefMut<Target = R> + 'a, Error> {
        let resource = self.try_resource()?.unwrap_or_default();
        Ok(ResourceProxy(self, resource))
    }

    pub fn attach_resource<R: Resource>(&self, resource: R) {
        self.try_attach_resource(resource).unwrap()
    }

    pub fn try_attach_resource<R: Resource>(&self, resource: R) -> Result<(), Error> {
        let name = R::component_name();
        let data = R::to_rusqlite(resource)?;

        self.conn.execute(
            "insert or replace into resources (name, data) values (?1, ?2)",
            params![name, data],
        )?;

        debug!(resource = name, "inserted");

        Ok(())
    }

    pub fn detach_resource<R: Resource>(&self) {
        self.try_detach_resource::<R>().unwrap()
    }

    pub fn try_detach_resource<R: Resource>(&self) -> Result<(), Error> {
        let name = R::component_name();

        self.conn
            .execute("delete from resources where name = ?1", params![name])?;

        debug!(resource = name, "deleted");

        Ok(())
    }
}

pub struct ResourceProxy<'a, R: Resource + Default>(&'a mut Ecs, R);

impl<'a, R: Resource + Default> AsMut<R> for ResourceProxy<'a, R> {
    fn as_mut(&mut self) -> &mut R {
        &mut self.1
    }
}

impl<'a, R: Resource + Default> Deref for ResourceProxy<'a, R> {
    type Target = R;

    fn deref(&self) -> &Self::Target {
        &self.1
    }
}

impl<'a, R: Resource + Default> DerefMut for ResourceProxy<'a, R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.1
    }
}

impl<'a, R: Resource + Default> Drop for ResourceProxy<'a, R> {
    fn drop(&mut self) {
        let resource = std::mem::take(&mut self.1);
        self.0.attach_resource(resource);
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use crate::{self as ecsdb};
    use crate::{Ecs, Resource}; // #[derive(Component)] derives `impl ecsdb::Component for ...`

    #[derive(Debug, Serialize, Deserialize, Resource, PartialEq, Default)]
    struct TestResource(pub i32);

    #[test]
    fn ecs_resource() {
        let mut ecs = Ecs::open_in_memory().unwrap();

        assert!(ecs.resource::<TestResource>().is_none());

        ecs.attach_resource(TestResource(42));
        assert_eq!(ecs.resource::<TestResource>().unwrap(), TestResource(42));

        ecs.attach_resource(TestResource(23));
        assert_eq!(ecs.resource::<TestResource>().unwrap(), TestResource(23));

        ecs.detach_resource::<TestResource>();
        assert!(ecs.resource::<TestResource>().is_none());

        ecs.resource_mut::<TestResource>().0 = 42;
        assert_eq!(ecs.resource::<TestResource>().unwrap(), TestResource(42));

        {
            let mut proxy = ecs.resource_mut::<TestResource>();
            *proxy = TestResource(1234);
        }

        assert_eq!(ecs.resource::<TestResource>().unwrap(), TestResource(1234));
    }
}
