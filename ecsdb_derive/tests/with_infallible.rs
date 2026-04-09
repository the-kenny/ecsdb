use ecsdb_derive::with_infallible;

#[derive(Debug, PartialEq)]
struct Foo;

#[derive(Debug)]
struct MyErr;

#[with_infallible]
impl Foo {
    pub fn try_double(&self, x: i32) -> Result<i32, MyErr> {
        Ok(x * 2)
    }

    pub fn try_consume_into<T>(self, _t: T) -> Result<Self, MyErr> {
        Ok(self)
    }

    pub fn try_borrow_with<'a>(&'a self, other: &'a str) -> Result<&'a str, MyErr> {
        Ok(other)
    }

    // non-pub: must be left alone (no `helper_no_prefix` companion generated)
    fn helper(&self) -> i32 {
        0
    }

    // pub but not `try_`-prefixed: ignored
    pub fn plain(&self) -> i32 {
        1
    }

    // `try_` prefixed but return type isn't `Result<_, _>`: ignored
    pub fn try_not_result(&self) -> i32 {
        2
    }
}

#[test]
fn generates_infallible_for_concrete_fn() {
    let f = Foo;
    assert_eq!(f.double(3), 6);
}

#[test]
fn generates_infallible_for_generic_fn_with_turbofish() {
    let out: Foo = Foo.consume_into::<u8>(0u8);
    assert_eq!(out, Foo);
}

#[test]
fn generates_infallible_for_borrowed_fn_with_lifetime() {
    let f = Foo;
    let s = "hello";
    assert_eq!(f.borrow_with(s), "hello");
}

#[with_infallible]
impl Foo {
    pub fn try_iter_items<'a, T: Clone>(
        &'a self,
        items: &'a [T],
    ) -> Result<impl Iterator<Item = T> + 'a + use<'a, T>, MyErr> {
        Ok(items.iter().cloned())
    }
}

#[test]
fn generates_infallible_for_impl_trait_with_use_capture() {
    let f = Foo;
    let xs = [1u8, 2, 3];
    let collected: Vec<u8> = f.iter_items(&xs).collect();
    assert_eq!(collected, vec![1, 2, 3]);
}

#[test]
fn leaves_untransformable_items_alone() {
    let f = Foo;
    // Originals still callable.
    assert_eq!(f.try_double(4).unwrap(), 8);
    assert_eq!(f.plain(), 1);
    assert_eq!(f.try_not_result(), 2);
    assert_eq!(f.helper(), 0);
}
