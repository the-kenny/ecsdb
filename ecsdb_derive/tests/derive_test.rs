// Shims
pub mod component {
    use crate::rusqlite;

    pub struct JsonStorage;
    pub struct BlobStorage;
    pub struct NullStorage;
    impl NullStorage {
        pub fn to_rusqlite<'a>(
            &'a self,
        ) -> Result<super::rusqlite::types::ToSqlOutput<'a>, StorageError> {
            todo!()
        }

        pub fn from_rusqlite(value: &rusqlite::types::ToSqlOutput<'_>) -> Result<(), StorageError> {
            todo!()
        }
    }

    pub trait Component {
        type Storage;
        const NAME: &'static str;

        fn component_name() -> &'static str {
            Self::NAME
        }
    }

    pub type StorageError = ();
    pub type BundleData<'a> = Vec<(&'static str, rusqlite::types::ToSqlOutput<'a>)>;
    pub type BundleDataRef<'a> = &'a [(&'static str, rusqlite::types::ToSqlOutput<'a>)];

    pub trait Bundle: Sized {
        const COMPONENTS: &'static [&'static str];

        fn component_names() -> &'static [&'static str] {
            Self::COMPONENTS
        }

        fn to_rusqlite<'a>(&'a self) -> Result<BundleData<'a>, StorageError>;
        // fn from_rusqlite<'a>(components: BundleDataRef<'a>) -> Result<Option<Self>, StorageError>;
    }

    pub trait ComponentWrite {}
}

pub mod resource {
    pub trait Resource: super::component::Component {
        fn resource_name() -> &'static str {
            <Self as super::component::Component>::component_name()
        }
    }
}

pub mod rusqlite {
    pub mod types {
        use std::marker::PhantomData;
        pub type ToSqlOutput<'a> = PhantomData<&'a ()>;
    }
}

// Necessary for development as we derive `ecsdb::Component for ...`
use crate as ecsdb;
use ecsdb::component::Component;
use ecsdb::resource::Resource;
use ecsdb_derive::{Bundle, Component, Resource};

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
fn derive_name_attribute() {
    #[derive(Component)]
    #[component(name = "foo::Bar")]
    struct X;

    assert_eq!(X::component_name(), "foo::Bar");

    #[derive(Resource)]
    #[component(name = "foo::Bar")]
    struct Y;

    assert_eq!(Y::component_name(), "foo::Bar");
}

#[test]
fn test_resource() {
    #[derive(Resource)]
    struct Foo;

    assert_eq!(Foo::resource_name(), "derive_test::Foo".to_string());
}

// #[test]
// fn derive_bundle_struct() {
//     #[derive(Debug, Component)]
//     struct A;

//     #[derive(Debug, Component)]
//     struct B;

//     #[derive(Debug, Bundle)]
//     struct Composed {
//         a: A,
//         b: B,
//     }

//     // Create Bundle from tuple
//     let _ = Composed::from((A, B));
// }
