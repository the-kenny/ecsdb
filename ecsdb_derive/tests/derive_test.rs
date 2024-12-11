struct JsonStorage;
struct BlobStorage;
struct NullStorage;
trait ComponentName {
    type Storage;

    fn component_name() -> &'static str;
}

#[test]
fn test_macro() {
    use crate as ecsdb; // Necessary for development as we derive `ecsdb::ComponentName for ...`
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
