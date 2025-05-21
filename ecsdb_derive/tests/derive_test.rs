// Shims
pub mod component {
    pub struct JsonStorage;
    pub struct BlobStorage;
    pub struct NullStorage;
    pub trait Component {
        type Storage;
        const NAME: &'static str;

        fn component_name() -> &'static str {
            Self::NAME
        }
    }
}

pub mod resource {
    pub trait Resource: super::component::Component {
        fn resource_name() -> &'static str {
            <Self as super::component::Component>::component_name()
        }
    }
}

// Necessary for development as we derive `ecsdb::Component for ...`
use crate as ecsdb;
use ecsdb::component::Component;
use ecsdb::resource::Resource;
use ecsdb_derive::{Component, Resource};

#[test]
fn test_component() {
    #[derive(Component)]
    #[component(storage = "json")]
    struct Foo;

    assert_eq!(Foo::component_name(), "derive_test::Foo".to_string());

    #[derive(Component)]
    #[component(storage = "blob")]
    struct Foo2(pub Vec<u8>);

    impl Into<Vec<u8>> for Foo2 {
        fn into(self) -> Vec<u8> {
            self.0
        }
    }

    impl From<Vec<u8>> for Foo2 {
        fn from(value: Vec<u8>) -> Self {
            Self(value)
        }
    }

    assert_eq!(Foo2::component_name(), "derive_test::Foo2".to_string());

    let _: Vec<u8> = Foo2(vec![]).into();
    let _ = Foo2::from(vec![]);

    #[derive(Component)]
    struct Unit;
}

#[test]
fn test_resource() {
    #[derive(Resource)]
    struct Foo;

    assert_eq!(Foo::resource_name(), "derive_test::Foo".to_string());
}
