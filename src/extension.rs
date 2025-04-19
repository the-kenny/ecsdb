use crate::{Ecs, SystemParam};

#[derive(Debug, thiserror::Error)]
#[error("Extension already registered")]
pub struct ExtensionExistsError;

pub trait Extension: Send {
    // fn create(ecs: &Ecs) -> Self;
}

impl<E: Extension + 'static> SystemParam for &E {
    type Item<'world> = &'world E;

    fn get_param<'world>(world: &'world Ecs, _system: &str) -> Self::Item<'world> {
        world.extension::<E>()
    }
}

impl Ecs {
    pub fn register_extension<E: Extension + 'static>(
        &mut self,
        extension: E,
    ) -> Result<(), ExtensionExistsError> {
        if self.extensions.contains::<E>() {
            Err(ExtensionExistsError)
        } else {
            self.extensions.insert(extension);
            Ok(())
        }
    }

    pub fn try_extension<'a, E: Extension + 'static>(&'a self) -> Option<&'a E> {
        self.extensions.get()
    }

    pub fn extension<'a, E: Extension + 'static>(&'a self) -> &'a E {
        self.try_extension().expect("Existing Extension")
    }
}

#[cfg(test)]
mod test {
    use crate::{Ecs, Extension};

    #[test]
    fn extension() {
        #[derive(PartialEq, Debug)]
        struct Test(String);
        impl Extension for Test {}

        let mut ecs = Ecs::open_in_memory().unwrap();
        ecs.register_extension(Test("foo".into())).unwrap();

        assert_eq!(ecs.try_extension::<Test>(), Some(&Test("foo".into())));
        assert_eq!(ecs.extension::<Test>(), &Test("foo".into()));

        struct Unregistered;
        impl Extension for Unregistered {}
        assert!(ecs.try_extension::<Unregistered>().is_none());
    }

    #[test]
    fn extension_system_param() {
        struct Test(i32);
        impl Extension for Test {}

        let mut ecs = Ecs::open_in_memory().unwrap();
        ecs.register_extension(Test(1234)).unwrap();

        fn sys(test: &Test) {
            assert_eq!(test.0, 1234);
        }

        ecs.register(sys);
        ecs.tick();
    }
}
