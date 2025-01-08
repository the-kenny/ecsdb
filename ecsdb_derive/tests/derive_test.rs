pub mod component {
    pub struct JsonStorage;
    pub struct BlobStorage;
    pub struct NullStorage;
    pub trait Component {
        type Storage;

        fn component_name() -> &'static str;
    }
}
pub mod resource {
    pub trait Resource: super::component::Component {
        fn resource_name() -> &'static str {
            <Self as super::component::Component>::component_name()
        }
    }
}

#[test]
fn test_component() {
    // Necessary for development as we derive `ecsdb::Component for ...`
    use crate as ecsdb;
    use ecsdb::component::Component;

    #[derive(ecsdb_derive::Component)]
    #[component(storage = "json")]
    struct Foo;

    assert_eq!(Foo::component_name(), "derive_test::Foo".to_string());

    #[derive(ecsdb_derive::Component)]
    #[component(storage = "blob")]
    struct Foo2(pub Vec<u8>);

    assert_eq!(Foo2::component_name(), "derive_test::Foo2".to_string());

    let _: Vec<u8> = Foo2(vec![]).into();
    let _ = Foo2::from(vec![]);

    #[derive(ecsdb_derive::Component)]
    struct Unit;
}

#[test]
fn test_resource() {
    // Necessary for development as we derive `ecsdb::Component for ...`
    use crate as ecsdb;
    use ecsdb::resource::Resource;

    #[derive(ecsdb_derive::Resource)]
    struct Foo;

    assert_eq!(Foo::resource_name(), "derive_test::Foo".to_string());
}
