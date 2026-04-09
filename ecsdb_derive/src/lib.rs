extern crate proc_macro;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, Expr, ExprLit, Fields, Lit, Meta, Token, parse_quote, punctuated::Punctuated,
};

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

/// Attribute macro for `impl` blocks that generates an infallible
/// companion for every `pub fn try_foo(...) -> Result<T, _>`.
///
/// For each matching method, appends a `pub fn foo(...) -> T {
/// self.try_foo(...).expect(stringify!(try_foo)) }`. Non-matching items (non-pub fns, fns whose
/// return type isn't a two-arg `Result`, non-fn items) get ignored.
#[proc_macro_attribute]
pub fn with_infallible(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut item_impl = syn::parse_macro_input!(item as syn::ItemImpl);
    let generated: Vec<syn::ImplItem> = item_impl
        .items
        .iter()
        .filter_map(infallible_compantion)
        .collect();
    item_impl.items.extend(generated);
    quote!(#item_impl).into()
}

fn infallible_compantion(item: &syn::ImplItem) -> Option<syn::ImplItem> {
    let syn::ImplItem::Fn(f) = item else {
        return None;
    };
    if !matches!(f.vis, syn::Visibility::Public(_)) {
        return None;
    }
    let sig = &f.sig;
    if sig.asyncness.is_some()
        || sig.constness.is_some()
        || sig.unsafety.is_some()
        || sig.abi.is_some()
    {
        return None;
    }

    let try_ident = sig.ident.clone();
    let stripped = try_ident.to_string();
    let stripped = stripped.strip_prefix("try_")?;
    if stripped.is_empty() {
        return None;
    }
    let new_ident = syn::Ident::new(stripped, try_ident.span());

    let syn::ReturnType::Type(_, ret_ty) = &sig.output else {
        return None;
    };
    let ok_ty = extract_ok_type(ret_ty)?.clone();

    let mut arg_idents: Vec<syn::Ident> = Vec::new();
    for input in sig.inputs.iter() {
        match input {
            syn::FnArg::Receiver(_) => {}
            syn::FnArg::Typed(pat_type) => {
                let syn::Pat::Ident(pat_ident) = &*pat_type.pat else {
                    return None;
                };
                arg_idents.push(pat_ident.ident.clone());
            }
        }
    }

    let type_params: Vec<&syn::Ident> = sig
        .generics
        .params
        .iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(tp) => Some(&tp.ident),
            _ => None,
        })
        .collect();
    let turbofish = if type_params.is_empty() {
        quote!()
    } else {
        quote!(::<#(#type_params),*>)
    };

    let mut new_sig = sig.clone();
    new_sig.ident = new_ident;
    new_sig.output = parse_quote!(-> #ok_ty);

    let mut new_fn: syn::ImplItemFn = parse_quote! {
        pub #new_sig {
            self.#try_ident #turbofish (#(#arg_idents),*).expect(stringify!(#try_ident))
        }
    };

    // Keep the documentation
    new_fn.attrs = f
        .attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .cloned()
        .collect();

    Some(syn::ImplItem::Fn(new_fn))
}

fn extract_ok_type(ty: &syn::Type) -> Option<&syn::Type> {
    let syn::Type::Path(tp) = ty else {
        return None;
    };
    let seg = tp.path.segments.last()?;
    if seg.ident != "Result" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let mut type_args = args.args.iter().filter_map(|a| match a {
        syn::GenericArgument::Type(t) => Some(t),
        _ => None,
    });
    let ok = type_args.next()?;

    type_args.next()?;
    if type_args.next().is_some() {
        return None;
    }
    Some(ok)
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

    if let Data::Struct(ref struc) = ast.data
        && matches!(struc.fields, Fields::Unit)
    {
        attributes.storage = Storage::Null;
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

fn impl_derive_bundle(ast: syn::DeriveInput) -> TokenStream {
    let name = ast.ident;

    match ast.data {
        Data::Struct(struc) => derive_bundle_for_struct(name, struc),
        Data::Enum(_) => panic!("Unsupported: Deriving Bundle for enum {name}"),
        Data::Union(_) => panic!("Unsupported: Deriving Bundle for union {name}"),
    }
}

fn derive_bundle_for_struct(name: syn::Ident, struc: syn::DataStruct) -> TokenStream {
    if struc.fields.is_empty() {
        return syn::Error::new_spanned(
               &name,
               "cannot derive `Bundle` on a struct with no fields: a Bundle must contain at least one component",
           )
           .to_compile_error()
           .into();
    }

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

        impl ecsdb::component::NonEmptyBundle for #name {}
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
                other => panic!(
                    "Unsupported attribute {}",
                    other.path().get_ident().unwrap()
                ),
            }
        }
    };

    attributes
}
