//! `#[derive(AbiType)]` describes a type to the ABI manifest.
//!
//! The shape it produces for each kind of type - field names, the nullability
//! rule, the synthesized payload records - is pinned by
//! `crates/sdk/tests/abi_derive_shapes.rs`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt;
use syn::{parse_quote, Data, DataEnum, DeriveInput, Error as SynError, Fields, Type};

use crate::doc;
use crate::errors::{Errors, ParseError};
use crate::serde_attrs::{self, ContainerAttrs, FieldAttrs, RenameRule};

pub fn derive(input: DeriveInput) -> TokenStream {
    let ident = &input.ident;
    // `#[abi(name = "...")]` picks the manifest name; the identifier is only
    // the default. This is how two types sharing an ident stay distinct.
    let options = match abi_options(&input.attrs) {
        Ok(options) => options,
        Err(err) => return compile_error(err),
    };
    let serde = match serde_attrs::container(&input.attrs) {
        Ok(serde) => serde,
        Err(err) => return compile_error(err),
    };
    let name = options.name.clone().unwrap_or_else(|| ident.to_string());
    let doc = doc::doc_text(&input.attrs);

    let body = match &input.data {
        Data::Struct(item) => struct_def(
            &item.fields,
            options.pattern.as_deref(),
            doc.as_deref(),
            &serde,
        ),
        Data::Enum(item) => enum_def(&name, item, doc.as_deref(), &serde),
        Data::Union(_) => Err(SynError::new_spanned(ident, ParseError::AbiTypeOnUnion)),
    };
    let body = match body {
        Ok(body) => body,
        Err(err) => return compile_error(err),
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

pub(crate) fn compile_error(err: SynError) -> TokenStream {
    let errors = Errors::default();
    errors.subsume(err);
    errors.to_compile_error()
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
fn struct_def(
    fields: &Fields,
    pattern: Option<&str>,
    doc: Option<&str>,
    serde: &ContainerAttrs,
) -> Result<TokenStream, SynError> {
    if let Some((key, span)) = &serde.tagging {
        return Err(SynError::new(
            *span,
            ParseError::UnsupportedSerdeAttr { attr: key.clone() },
        ));
    }

    let doc = doc::tokens(doc);
    if let Fields::Unnamed(unnamed) = fields {
        if unnamed.unnamed.len() == 1 {
            let field = &unnamed.unnamed[0];
            let ty = wire_type(field, &serde_attrs::field(&field.attrs)?)?;
            let pattern = option_string(pattern);
            return Ok(quote! {
                ::calimero_sdk::abi::TypeDef::Alias {
                    doc: #doc,
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

    let fields = fields_vec(fields, false, serde.rename_all)?;
    Ok(quote! {
        ::calimero_sdk::abi::TypeDef::Record {
            doc: #doc,
            fields: #fields,
        }
    })
}

fn enum_def(
    enum_name: &str,
    data: &DataEnum,
    doc: Option<&str>,
    serde: &ContainerAttrs,
) -> Result<TokenStream, SynError> {
    let mut synthesized = Vec::new();
    let mut variants = Vec::new();
    for variant in &data.variants {
        let attrs = serde_attrs::variant(&variant.attrs)?;
        if attrs.skip {
            continue;
        }
        let name = serde.variant_name(&variant.ident, &attrs);
        let field_rule = attrs.rename_all.or(serde.rename_all_fields);
        let payload = variant_payload(enum_name, variant, field_rule, &mut synthesized)?;
        let variant_doc = doc::tokens(doc::doc_text(&variant.attrs).as_deref());
        variants.push(quote! {
            ::calimero_sdk::abi::Variant {
                name: #name.to_owned(),
                code: ::core::option::Option::None,
                payload: #payload,
                doc: #variant_doc,
            }
        });
    }

    let doc = doc::tokens(doc);
    let tag = option_string(serde.tag.as_deref());
    let content = option_string(serde.content.as_deref());
    let untagged = serde.untagged;
    Ok(quote! {
        #(#synthesized)*
        ::calimero_sdk::abi::TypeDef::Variant {
            doc: #doc,
            variants: ::std::vec![#(#variants),*],
            tag: #tag,
            content: #content,
            untagged: #untagged,
        }
    })
}

fn option_string(value: Option<&str>) -> TokenStream {
    match value {
        Some(value) => quote! { ::core::option::Option::Some(#value.to_owned()) },
        None => quote! { ::core::option::Option::None },
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
    field_rule: Option<RenameRule>,
    synthesized: &mut Vec<TokenStream>,
) -> Result<TokenStream, SynError> {
    if variant.fields.is_empty() {
        return Ok(quote! { ::core::option::Option::None });
    }

    if let Fields::Unnamed(unnamed) = &variant.fields {
        if unnamed.unnamed.len() == 1 {
            let field = &unnamed.unnamed[0];
            let ty = wire_type(field, &serde_attrs::field(&field.attrs)?)?;
            return Ok(quote! {
                ::core::option::Option::Some(
                    <#ty as ::calimero_sdk::abi::AbiType>::type_ref(__reg)
                )
            });
        }
    }

    let record = format!("{}_{}", enum_name, variant.ident);
    let fields = fields_vec(&variant.fields, true, field_rule)?;
    synthesized.push(quote! {
        __reg.define(#record, |__reg| ::calimero_sdk::abi::TypeDef::Record {
            doc: ::core::option::Option::None,
            fields: #fields,
        });
    });

    Ok(quote! {
        ::core::option::Option::Some(::calimero_sdk::abi::TypeRef::reference(#record))
    })
}

/// The `Field` list for a record, marking `Option` fields nullable. A payload
/// record (synthesized from an enum variant) names its tuple fields `field_{i}`;
/// a struct's own record names every tuple field `unnamed`.
fn fields_vec(
    fields: &Fields,
    payload: bool,
    rule: Option<RenameRule>,
) -> Result<TokenStream, SynError> {
    let mut entries = Vec::new();
    for (index, field) in fields.iter().enumerate() {
        let serde = serde_attrs::field(&field.attrs)?;
        if serde.skip {
            continue;
        }
        let name = match (&serde.rename, &field.ident) {
            (Some(rename), _) => rename.clone(),
            (None, Some(ident)) => {
                let ident = ident.unraw().to_string();
                rule.map_or_else(|| ident.clone(), |rule| rule.apply_to_field(&ident))
            }
            (None, None) if payload => format!("field_{index}"),
            (None, None) => "unnamed".to_owned(),
        };
        let ty = wire_type(field, &serde)?;
        let doc = doc::tokens(doc::doc_text(&field.attrs).as_deref());
        let nullable = nullable(&ty);
        entries.push(quote! {
            ::calimero_sdk::abi::Field {
                name: #name.to_owned(),
                type_: <#ty as ::calimero_sdk::abi::AbiType>::type_ref(__reg),
                nullable: #nullable,
                doc: #doc,
            }
        });
    }

    Ok(quote! { ::std::vec![#(#entries),*] })
}

/// The type a field has on the wire: `#[abi(as = T)]` when given, which a field
/// written by a hand-written serde function must declare.
fn wire_type(field: &syn::Field, serde: &FieldAttrs) -> Result<Type, SynError> {
    let declared = abi_as(&field.attrs)?;
    if let (Some((key, span)), None) = (&serde.custom_wire, &declared) {
        return Err(SynError::new(
            *span,
            ParseError::SerdeWireNeedsAbiAs { attr: key.clone() },
        ));
    }
    Ok(declared.unwrap_or_else(|| field.ty.clone()))
}

fn abi_as(attrs: &[syn::Attribute]) -> Result<Option<Type>, SynError> {
    let Some(attr) = attrs.iter().find(|attr| attr.path().is_ident("abi")) else {
        return Ok(None);
    };
    let mut declared = None;
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("as") {
            declared = Some(meta.value()?.parse::<Type>()?);
            Ok(())
        } else {
            Err(meta.error("unsupported field `abi` key; expected `as = WireType`"))
        }
    })?;
    Ok(declared)
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

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::*;

    fn expand(ts: TokenStream) -> String {
        derive(syn::parse2(ts).expect("parse DeriveInput")).to_string()
    }

    #[test]
    fn a_tagged_struct_is_refused() {
        let out = expand(quote! {
            #[serde(tag = "kind")]
            struct Tagged { id: u32 }
        });
        assert!(
            out.contains("`#[serde(tag)]` changes the JSON wire shape"),
            "{out}"
        );

        let out = expand(quote! {
            #[serde(untagged)]
            struct Untagged { id: u32 }
        });
        assert!(
            out.contains("`#[serde(untagged)]` changes the JSON wire shape"),
            "{out}"
        );
    }

    #[test]
    fn a_variant_serde_key_the_abi_cannot_describe_is_refused() {
        let out = expand(quote! {
            enum Kind {
                Known,
                #[serde(other)]
                Unknown,
            }
        });
        assert!(
            out.contains("`#[serde(other)]` changes the JSON wire shape"),
            "{out}"
        );
    }

    #[test]
    fn a_custom_serialized_newtype_variant_needs_abi_as() {
        let out = expand(quote! {
            enum Id {
                Hex(#[serde(serialize_with = "as_hex")] [u8; 2]),
            }
        });
        assert!(
            out.contains("`#[serde(serialize_with)]` hides this field's wire type"),
            "{out}"
        );
    }

    #[test]
    fn a_field_abi_key_other_than_as_is_refused() {
        let out = expand(quote! {
            struct Blob {
                #[abi(name = "x")]
                id: u32,
            }
        });
        assert!(out.contains("expected `as = WireType`"), "{out}");
    }
}
