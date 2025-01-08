extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;
use syn::{punctuated::Punctuated, Data, Expr, Fields, Lit, Meta, Token};

#[proc_macro_derive(Component, attributes(component))]
pub fn derive_component_fn(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    impl_derive_macro(&ast)
}

#[derive(Debug)]
enum Storage {
    Json,
    Blob,
    Null,
}

fn impl_derive_macro(ast: &syn::DeriveInput) -> TokenStream {
    let name = ast.ident.clone();

    let mut storage = Storage::Json;

    if let Data::Struct(ref struc) = ast.data {
        if matches!(struc.fields, Fields::Unit) {
            storage = Storage::Null;
        }
    }

    if let Some(component_attribute) = ast.attrs.iter().find(|a| a.path().is_ident("component")) {
        let nested = component_attribute
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .unwrap();
        for meta in nested.clone() {
            match meta {
                Meta::NameValue(mnv) if mnv.path.is_ident("storage") => {
                    if let Expr::Lit(expr_lit) = &mnv.value {
                        if let Lit::Str(lit) = &expr_lit.lit {
                            match lit.value().as_str() {
                                "json" => storage = Storage::Json,
                                "blob" => storage = Storage::Blob,
                                other => panic!("storage {other} not supported"),
                            }
                        }
                    }
                }
                other => panic!(
                    "Unsupported attribute {}",
                    other.path().get_ident().unwrap()
                ),
            }
        }
    };

    let gen = match storage {
        Storage::Json => {
            quote! {
                    impl ecsdb::Component for #name {
                        type Storage =ecsdb::component::JsonStorage;

                        fn component_name() -> &'static str {
                            concat!(std::module_path!(), "::", stringify!(#name))
                        }
                    }
            }
        }

        Storage::Null => {
            quote! {
                    impl ecsdb::Component for #name {
                        type Storage = ecsdb::component::NullStorage;

                        fn component_name() -> &'static str {
                            concat!(std::module_path!(), "::", stringify!(#name))
                        }
                    }
            }
        }

        Storage::Blob => {
            quote! {
                impl ecsdb::Component for #name {
                    type Storage = ecsdb::component::BlobStorage;

                    fn component_name() -> &'static str {
                        concat!(std::module_path!(), "::", stringify!(#name))
                    }
                }

                impl Into<Vec<u8>> for #name {
                    fn into(self) -> Vec<u8> {
                        self.0
                    }
                }

                impl From<Vec<u8>> for #name {
                    fn from(value: Vec<u8>) -> Self {
                        Self(value)
                    }
                }
            }
        }
    };

    gen.into()
}
