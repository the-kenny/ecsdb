extern crate proc_macro;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{punctuated::Punctuated, Attribute, Data, Expr, Fields, Lit, Meta, Token};

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

#[proc_macro_derive(Bundle)]
pub fn derive_bundle_fn(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    impl_derive_bundle(ast)
}

#[derive(Debug, Default)]
enum Storage {
    #[default]
    Json,
    Blob,
    Null,
}

#[derive(Debug, Default)]
enum Name {
    #[default]
    Derived,
    Custom(String),
}

#[derive(Debug, Default)]
struct Attributes {
    storage: Storage,
    name: Name,
}

fn impl_derive_component(ast: &syn::DeriveInput) -> TokenStream {
    let name = ast.ident.clone();

    let mut attributes = extract_attributes(&ast.attrs);

    if let Data::Struct(ref struc) = ast.data {
        if matches!(struc.fields, Fields::Unit) {
            attributes.storage = Storage::Null;
        }
    }

    let component_name = match attributes.name {
        Name::Derived => quote!(concat!(std::module_path!(), "::", stringify!(#name))),
        Name::Custom(name) => quote!(#name),
    };

    let storage = match attributes.storage {
        Storage::Json => quote!(ecsdb::component::JsonStorage),
        Storage::Blob => quote!(ecsdb::component::BlobStorage),
        Storage::Null => quote!(ecsdb::component::NullStorage),
    };

    quote! {
        impl ecsdb::component::Component for #name {
            type Storage = #storage;
            const NAME: &'static str = #component_name;
        }
    }
    .into()
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

fn impl_derive_bundle(ast: syn::DeriveInput) -> TokenStream {
    let name = ast.ident;

    match ast.data {
        Data::Struct(struc) => derive_bundle_for_struct(name, struc),
        Data::Enum(_) => panic!("Unsupported: Deriving Bundle for enum {name}"),
        Data::Union(_) => panic!("Unsupported: Deriving Bundle for union {name}"),
    }
}

fn derive_bundle_for_struct(name: syn::Ident, struc: syn::DataStruct) -> TokenStream {
    let types = struc.fields.iter().map(|f| &f.ty).collect::<Vec<_>>();

    let field_names: Vec<_> = struc.fields.members().collect();

    let field_vars: Vec<_> = struc
        .fields
        .members()
        .enumerate()
        .map(|(idx, m)| match m {
            syn::Member::Named(ident) => ident,
            syn::Member::Unnamed(_) => format_ident!("f{idx}"),
        })
        .collect();

    // Either `a` for structs or `a: f_0` for tuple-structs. Lhs is from,
    // `field_names` rhs from `field_vars`.
    //
    // Example: `let Self { field_bindings.. } = ...;`
    let field_bindings: Vec<_> = field_names
        .iter()
        .zip(field_vars.iter())
        .map(|(name, var)| {
            if *name == syn::Member::from(var.clone()) {
                quote!(#name)
            } else {
                quote!(#name: #var)
            }
        })
        .collect();

    quote! {
        impl ecsdb::component::Bundle for #name {
            const COMPONENTS: &'static [&'static str] = &[
                #(#types::NAME),*
            ];

            fn to_rusqlite<'a>(
                &'a self,
            ) -> Result<ecsdb::component::BundleData<'a>, ecsdb::component::StorageError> {
                let Self { #(#field_bindings,)* } = self;

                use ecsdb::component::ComponentWrite;

                Ok(vec![
                    #(
                        (
                            <#types as ecsdb::Component>::NAME,
                            <#types as ecsdb::Component>::Storage::to_rusqlite(#field_vars)?
                        ),
                    )*
                ])
            }

            fn from_rusqlite<'a>(
                components: ecsdb::component::BundleDataRef<'a>,
            ) -> Result<Option<Self>, ecsdb::component::StorageError> {
                let (#(Some(#field_vars),)*) = (#(<#types as ecsdb::Bundle>::from_rusqlite(components)?),*) else {
                    return Ok(None);
                };

                Ok(Some(Self { #(#field_bindings,)* }))
            }
    }
    }.into()
}

fn extract_attributes(attrs: &[Attribute]) -> Attributes {
    let mut attributes = Attributes::default();

    if let Some(component_attribute) = attrs.into_iter().find(|a| a.path().is_ident("component")) {
        let nested = component_attribute
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .unwrap();

        for meta in nested {
            match meta {
                Meta::NameValue(mnv) if mnv.path.is_ident("storage") => {
                    if let Expr::Lit(expr_lit) = &mnv.value {
                        if let Lit::Str(lit) = &expr_lit.lit {
                            match lit.value().as_str() {
                                "json" => attributes.storage = Storage::Json,
                                "blob" => attributes.storage = Storage::Blob,
                                other => panic!("storage {other} not supported"),
                            }
                        }
                    }
                }
                Meta::NameValue(mnv) if mnv.path.is_ident("name") => {
                    if let Expr::Lit(expr_lit) = &mnv.value {
                        if let Lit::Str(lit) = &expr_lit.lit {
                            let custom_name = lit.value();
                            attributes.name = Name::Custom(custom_name);
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

    attributes
}
