trait ComponentName {
    fn component_name() -> &'static str;
}

#[test]
fn test_macro() {
    use crate as ecsdb; // Necessary for development as we derive `ecsdb::ComponentName for ...`
    #[derive(ecsdb_derive::Component)]
    struct Foo;

    assert_eq!(Foo::component_name(), "derive_test::Foo".to_string());
}
