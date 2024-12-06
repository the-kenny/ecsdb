extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;

#[proc_macro_derive(Component)]
pub fn derive_component_fn(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    impl_hello_macro(&ast)
}

fn impl_hello_macro(ast: &syn::DeriveInput) -> TokenStream {
    let name = ast.ident.clone();

    let gen = quote! {
        impl ecsdb::ComponentName for #name {
            fn component_name() -> &'static str {
                concat!(std::module_path!(), "::", stringify!(#name))
            }
        }
    };

    gen.into()
}
