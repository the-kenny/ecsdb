use std::ops::{Deref, DerefMut};

use ecsdb_derive::with_infallible;

use crate::{Component, Ecs, Error};

#[with_infallible]
impl Ecs {
    pub fn try_resource<R: Component>(&self) -> Result<Option<R>, Error> {
        self.world_entity().try_component::<R>()
    }

    pub fn try_resource_mut<'a, R: Component + Default>(
        &'a mut self,
    ) -> Result<impl DerefMut<Target = R> + 'a, Error> {
        let resource = self.try_resource()?.unwrap_or_default();
        Ok(ResourceProxy(self, resource))
    }

    pub fn try_attach_resource<R: Component>(&self, resource: R) -> Result<(), Error> {
        self.world_entity().try_attach(resource)?;
        Ok(())
    }

    pub fn try_detach_resource<R: Component>(&self) -> Result<(), Error> {
        self.world_entity().try_detach::<R>()?;
        Ok(())
    }
}

pub struct ResourceProxy<'a, R: Component + Default>(&'a mut Ecs, R);

impl<'a, R: Component + Default> AsMut<R> for ResourceProxy<'a, R> {
    fn as_mut(&mut self) -> &mut R {
        &mut self.1
    }
}

impl<'a, R: Component + Default> Deref for ResourceProxy<'a, R> {
    type Target = R;

    fn deref(&self) -> &Self::Target {
        &self.1
    }
}

impl<'a, R: Component + Default> DerefMut for ResourceProxy<'a, R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.1
    }
}

impl<'a, R: Component + Default> Drop for ResourceProxy<'a, R> {
    fn drop(&mut self) {
        let resource = std::mem::take(&mut self.1);
        self.0.attach_resource(resource);
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use crate::{self as ecsdb};
    use crate::{Component, Ecs};

    #[derive(Debug, Serialize, Deserialize, Component, PartialEq, Default)]
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
