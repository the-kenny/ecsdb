extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;
use syn::{punctuated::Punctuated, Data, Expr, Fields, Lit, Meta, Token};

#[proc_macro_derive(Component, attributes(component))]
pub fn derive_component_fn(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    impl_derive_component(&ast)
}

#[proc_macro_derive(Resource, attributes(component))]
pub fn derive_resource_fn(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    impl_derive_resource(&ast)
}

#[derive(Debug)]
enum Storage {
    Json,
    Blob,
    Null,
}

fn impl_derive_component(ast: &syn::DeriveInput) -> TokenStream {
    let name = ast.ident.clone();

    let mut storage = Storage::Json;
    let mut component_name = quote!(concat!(std::module_path!(), "::", stringify!(#name)));

    if let Data::Struct(ref struc) = ast.data {
        if matches!(struc.fields, Fields::Unit) {
            storage = Storage::Null;
        }
    }

    if let Some(component_attribute) = ast.attrs.iter().find(|a| a.path().is_ident("component")) {
        let nested = component_attribute
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .unwrap();

        for meta in nested {
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
                Meta::NameValue(mnv) if mnv.path.is_ident("name") => {
                    if let Expr::Lit(expr_lit) = &mnv.value {
                        if let Lit::Str(lit) = &expr_lit.lit {
                            let custom_name = lit.value();
                            component_name = quote!(#custom_name);
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
                impl ecsdb::component::Component for #name {
                    type Storage = ecsdb::component::JsonStorage;
                    const NAME: &'static str = #component_name;
                }
            }
        }

        Storage::Null => {
            quote! {
                impl ecsdb::component::Component for #name {
                    type Storage = ecsdb::component::NullStorage;
                    const NAME: &'static str = #component_name;
                }
            }
        }

        Storage::Blob => {
            quote! {
                impl ecsdb::component::Component for #name {
                    type Storage = ecsdb::component::BlobStorage;
                    const NAME: &'static str = #component_name;
                }
            }
        }
    };

    gen.into()
}

fn impl_derive_resource(ast: &syn::DeriveInput) -> TokenStream {
    let name = ast.ident.clone();

    let component_derive: proc_macro2::TokenStream = impl_derive_component(ast).into();

    let gen = quote! {
        #component_derive

        impl ecsdb::resource::Resource for #name { }
    };

    gen.into()
}
