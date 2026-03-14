extern crate proc_macro;

use std::collections::HashSet;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Attribute, Data, Expr, ExprLit, Fields, Lit, Meta, Token, punctuated::Punctuated};

#[proc_macro_derive(Component, attributes(component))]
pub fn derive_component_fn(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    impl_derive_component(&ast)
}

/// Deprecated alias for `#[derive(Component)]`. Do not combine with `Component`.
#[proc_macro_derive(Resource, attributes(component))]
pub fn derive_resource_fn(input: TokenStream) -> TokenStream {
    let ast: syn::DeriveInput = syn::parse(input).unwrap();
    let name = &ast.ident;
    let marker = format_ident!("_ecsdb_Resource_deprecated_for_{}", name);
    let component_impl: proc_macro2::TokenStream = impl_derive_component(&ast).into();
    quote! {
        #component_impl

        #[allow(non_camel_case_types)]
        #[deprecated(note = "use `#[derive(Component)]` instead of `#[derive(Resource)]`")]
        struct #marker;
        const _: #marker = #marker;
    }
    .into()
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
    other_names: HashSet<String>,
}

fn impl_derive_component(ast: &syn::DeriveInput) -> TokenStream {
    let name = ast.ident.clone();

    let mut attributes = extract_attributes(&ast.attrs);

    if let Data::Struct(ref struc) = ast.data
        && matches!(struc.fields, Fields::Unit)
    {
        attributes.storage = Storage::Null;
    }

    let component_name = match attributes.name {
        Name::Derived => quote!(concat!(std::module_path!(), "::", stringify!(#name))),
        Name::Custom(name) => quote!(#name),
    };

    let other_component_names = attributes
        .other_names
        .into_iter()
        .map(|name| quote!(#name))
        .collect::<Vec<_>>();
    let other_component_names = quote!(&[#(#other_component_names),*]);

    let storage = match attributes.storage {
        Storage::Json => quote!(ecsdb::component::JsonStorage),
        Storage::Blob => quote!(ecsdb::component::BlobStorage),
        Storage::Null => quote!(ecsdb::component::NullStorage),
    };

    quote! {
        impl ecsdb::component::Component for #name {
            type Storage = #storage;
            const NAME: &'static str = #component_name;
            const OTHER_NAMES: &'static [&'static str] = #other_component_names;
        }
    }
    .into()
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
                #(<#types as ecsdb::component::BundleComponent>::NAME),*
            ];

            fn to_rusqlite<'a>(
                &'a self,
            ) -> Result<ecsdb::component::BundleData<'a>, ecsdb::component::StorageError> {
                let Self { #(#field_bindings,)* } = self;

                use ecsdb::component::ComponentWrite;

                Ok(vec![
                    #(
                        (
                            <#types as ecsdb::component::BundleComponent>::NAME,
                            <#types as ecsdb::component::BundleComponent>::to_rusqlite(#field_vars)?
                        ),
                    )*
                ])
            }
        }
    }
    .into()
}

fn extract_attributes(attrs: &[Attribute]) -> Attributes {
    let mut attributes = Attributes::default();

    if let Some(component_attribute) = attrs.iter().find(|a| a.path().is_ident("component")) {
        let nested = component_attribute
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .unwrap();

        for meta in nested {
            match meta {
                Meta::NameValue(mnv) if mnv.path.is_ident("storage") => {
                    if let Expr::Lit(expr_lit) = &mnv.value
                        && let Lit::Str(lit) = &expr_lit.lit
                    {
                        match lit.value().as_str() {
                            "json" => attributes.storage = Storage::Json,
                            "blob" => attributes.storage = Storage::Blob,
                            other => panic!("storage {other} not supported"),
                        }
                    }
                }
                Meta::NameValue(mnv) if mnv.path.is_ident("name") => {
                    if let Expr::Lit(expr_lit) = &mnv.value
                        && let Lit::Str(lit) = &expr_lit.lit
                    {
                        let custom_name = lit.value();
                        attributes.name = Name::Custom(custom_name);
                    }
                }
                Meta::NameValue(mnv) if mnv.path.is_ident("other_names") => {
                    let Expr::Array(array_expr) = &mnv.value else {
                        panic!("other_names must be an array of string-literals");
                    };

                    attributes.other_names = array_expr
                        .elems
                        .iter()
                        .filter_map(|e| match e {
                            Expr::Lit(ExprLit {
                                lit: Lit::Str(lit), ..
                            }) => Some(lit.value()),
                            _ => None,
                        })
                        .collect();
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
