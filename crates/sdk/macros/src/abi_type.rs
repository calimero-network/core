//! `#[derive(AbiType)]` describes a type to the ABI manifest.
//!
//! The shape it produces for each kind of type - field names, the nullability
//! rule, the synthesized payload records - is pinned by
//! `crates/sdk/tests/abi_derive_shapes.rs`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse_quote, Data, DataEnum, DeriveInput, Error as SynError, Fields, Type};

use crate::errors::{Errors, ParseError};

pub fn derive(input: DeriveInput) -> TokenStream {
    let ident = &input.ident;
    // `#[abi(name = "...")]` picks the manifest name; the identifier is only
    // the default. This is how two types sharing an ident stay distinct.
    let options = match abi_options(&input.attrs) {
        Ok(options) => options,
        Err(err) => {
            let errors = Errors::default();
            errors.subsume(err);
            return errors.to_compile_error();
        }
    };
    let name = options.name.clone().unwrap_or_else(|| ident.to_string());

    let body = match &input.data {
        Data::Struct(item) => match struct_def(&item.fields, options.pattern.as_deref()) {
            Ok(body) => body,
            Err(err) => {
                let errors = Errors::default();
                errors.subsume(err);
                return errors.to_compile_error();
            }
        },
        Data::Enum(item) => enum_def(&name, item),
        Data::Union(_) => {
            let errors = Errors::default();
            errors.subsume(SynError::new_spanned(ident, ParseError::AbiTypeOnUnion));
            return errors.to_compile_error();
        }
    };

    // Lifetimes and const params ride through `split_for_impl` untouched; only
    // type params need the recursive bound.
    let mut generics = input.generics.clone();
    for param in input.generics.type_params() {
        let param = &param.ident;
        generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#param: ::calimero_sdk::abi::AbiType));
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // ABI description runs only on the host (extraction, tests). Compiling it
    // into the wasm is dead weight that pushed unoptimized profiling builds
    // past the runtime's module size limit.
    quote! {
        #[cfg(not(target_arch = "wasm32"))]
        impl #impl_generics ::calimero_sdk::abi::AbiType for #ident #ty_generics #where_clause {
            fn type_ref(
                __reg: &mut ::calimero_sdk::abi::TypeRegistry,
            ) -> ::calimero_sdk::abi::TypeRef {
                <Self as ::calimero_sdk::abi::AbiType>::register(__reg);
                ::calimero_sdk::abi::TypeRef::reference(#name)
            }

            fn register(__reg: &mut ::calimero_sdk::abi::TypeRegistry) {
                __reg.define(#name, |__reg| { #body });
            }
        }
    }
}

/// The `#[abi(...)]` options, if present and well-formed. A rename gives a type
/// a manifest identity independent of its Rust identifier, which is how two
/// types sharing an ident register without colliding. A `pattern` constrains a
/// newtype's values for generated clients; it is descriptive, not enforced here.
#[derive(Default)]
struct AbiOptions {
    name: Option<String>,
    pattern: Option<String>,
}

fn abi_options(attrs: &[syn::Attribute]) -> Result<AbiOptions, SynError> {
    let mut options = AbiOptions::default();
    let Some(attr) = attrs.iter().find(|attr| attr.path().is_ident("abi")) else {
        return Ok(options);
    };

    attr.parse_nested_meta(|meta| {
        let key = if meta.path.is_ident("name") {
            &mut options.name
        } else if meta.path.is_ident("pattern") {
            &mut options.pattern
        } else {
            return Err(meta
                .error("unsupported `abi` key; expected `name = \"...\"` or `pattern = \"...\"`"));
        };
        let value = meta.value()?.parse::<syn::LitStr>()?.value();
        if value.is_empty() {
            return Err(meta.error("value must not be empty"));
        }
        *key = Some(value);
        Ok(())
    })?;

    if options.name.is_none() && options.pattern.is_none() {
        return Err(SynError::new_spanned(
            attr,
            "`#[abi(...)]` requires `name = \"...\"` or `pattern = \"...\"`",
        ));
    }

    Ok(options)
}

/// A one-field tuple struct is an alias to its inner type; everything else
/// (including a unit struct) is a record.
fn struct_def(fields: &Fields, pattern: Option<&str>) -> Result<TokenStream, SynError> {
    if let Fields::Unnamed(unnamed) = fields {
        if unnamed.unnamed.len() == 1 {
            let ty = &unnamed.unnamed[0].ty;
            let pattern = match pattern {
                Some(pattern) => quote!(Some(#pattern.to_owned())),
                None => quote!(None),
            };
            return Ok(quote! {
                ::calimero_sdk::abi::TypeDef::Alias {
                    target: <#ty as ::calimero_sdk::abi::AbiType>::type_ref(__reg),
                    pattern: #pattern,
                }
            });
        }
    }

    if let Some(pattern) = pattern {
        return Err(SynError::new_spanned(
            proc_macro2::Literal::string(pattern),
            "`pattern` applies only to a one-field tuple struct",
        ));
    }

    let fields = fields_vec(fields, false);
    Ok(quote! { ::calimero_sdk::abi::TypeDef::Record { fields: #fields } })
}

fn enum_def(enum_name: &str, data: &DataEnum) -> TokenStream {
    let mut synthesized = Vec::new();
    let variants: Vec<_> = data
        .variants
        .iter()
        .map(|variant| {
            let name = variant.ident.to_string();
            let payload = variant_payload(enum_name, variant, &mut synthesized);
            quote! {
                ::calimero_sdk::abi::Variant {
                    name: #name.to_owned(),
                    code: ::core::option::Option::None,
                    payload: #payload,
                }
            }
        })
        .collect();

    quote! {
        #(#synthesized)*
        ::calimero_sdk::abi::TypeDef::Variant {
            variants: ::std::vec![#(#variants),*],
        }
    }
}

/// The payload `TypeRef` expression for one variant, pushing the `define` call
/// for a synthesized `{Enum}_{Variant}` record when the shape needs one. Shared
/// with the `AbiEvents` codegen so an event variant describes identically.
///
/// Both the emitted statements and the expression read a registry bound as
/// `__reg` at the call site.
pub(crate) fn variant_payload(
    enum_name: &str,
    variant: &syn::Variant,
    synthesized: &mut Vec<TokenStream>,
) -> TokenStream {
    if variant.fields.is_empty() {
        return quote! { ::core::option::Option::None };
    }

    if let Fields::Unnamed(unnamed) = &variant.fields {
        if unnamed.unnamed.len() == 1 {
            let ty = &unnamed.unnamed[0].ty;
            return quote! {
                ::core::option::Option::Some(
                    <#ty as ::calimero_sdk::abi::AbiType>::type_ref(__reg)
                )
            };
        }
    }

    let record = format!("{}_{}", enum_name, variant.ident);
    let fields = fields_vec(&variant.fields, true);
    synthesized.push(quote! {
        __reg.define(#record, |__reg| ::calimero_sdk::abi::TypeDef::Record { fields: #fields });
    });

    quote! {
        ::core::option::Option::Some(::calimero_sdk::abi::TypeRef::reference(#record))
    }
}

/// The `Field` list for a record. A payload record (synthesized from an enum
/// variant) names its tuple fields `field_{i}` and is never nullable; a
/// struct's own record names every tuple field `unnamed` and marks `Option`
/// fields nullable. Both replicate the emitter.
fn fields_vec(fields: &Fields, payload: bool) -> TokenStream {
    let entries = fields.iter().enumerate().map(|(index, field)| {
        let name = field.ident.as_ref().map_or_else(
            || {
                if payload {
                    format!("field_{index}")
                } else {
                    "unnamed".to_owned()
                }
            },
            ToString::to_string,
        );
        let ty = &field.ty;
        let nullable = if payload {
            quote! { ::core::option::Option::None }
        } else {
            nullable(&field.ty)
        };
        quote! {
            ::calimero_sdk::abi::Field {
                name: #name.to_owned(),
                type_: <#ty as ::calimero_sdk::abi::AbiType>::type_ref(__reg),
                nullable: #nullable,
            }
        }
    });

    quote! { ::std::vec![#(#entries),*] }
}

/// A use site is nullable only when it is written as a 1-segment `Option<..>`
/// (so `std::option::Option` is not), and never carries `Some(false)`. Shared
/// with the logic codegen, which applies the same rule to params and returns.
pub(crate) fn nullable(ty: &Type) -> TokenStream {
    // References are transparent to the described type, so `&Option<T>` is as
    // nullable as `Option<T>`.
    let mut ty = ty;
    while let Type::Reference(reference) = ty {
        ty = &reference.elem;
    }
    let is_option = matches!(
        ty,
        Type::Path(path)
            if path.path.segments.len() == 1 && path.path.segments[0].ident == "Option"
    );

    if is_option {
        quote! { ::core::option::Option::Some(true) }
    } else {
        quote! { ::core::option::Option::None }
    }
}
