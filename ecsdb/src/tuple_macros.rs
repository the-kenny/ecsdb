macro_rules! for_each_tuple {
    ( $m:ident; ) => { };

    ( $m:ident; $h:ident, $($t:ident,)* ) => (
        $m!($h $($t)*);
        crate::tuple_macros::for_each_tuple! { $m; $($t,)* }
    );

    ( $m:ident ) => {
        crate::tuple_macros::for_each_tuple! { $m; A, B, C, D, E, F, G, H, I, J, K, L, M, O, P, Q, }
    };
}

pub(crate) use for_each_tuple;
