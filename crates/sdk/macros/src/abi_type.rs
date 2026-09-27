//! `#[derive(AbiType)]` describes a type to the ABI manifest.
//!
//! The shape it produces for each kind of type - field names, the nullability
//! rule, the synthesized payload records - is pinned by
//! `crates/sdk/tests/abi_derive_shapes.rs`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt;
use syn::{
    parse_quote, Data, DataEnum, DeriveInput, Error as SynError, Fields, FieldsNamed,
    FieldsUnnamed, Type,
};

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

/// A one-field tuple struct is an alias to its inner type, a wider one an alias to
/// a `tuple`; a unit or zero-field struct is an empty record.
fn struct_def(
    fields: &Fields,
    pattern: Option<&str>,
    doc: Option<&str>,
    serde: &ContainerAttrs,
) -> Result<TokenStream, SynError> {
    let tagging = (serde.tag.as_ref().map(|(_, span)| ("tag", *span)))
        .or(serde.untagged.map(|span| ("untagged", span)));
    if let Some((key, span)) = tagging {
        return Err(SynError::new(
            span,
            ParseError::UnsupportedSerdeAttr {
                attr: key.to_owned(),
            },
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

    match fields {
        Fields::Named(named) => {
            let fields = fields_vec(named, serde.rename_all)?;
            Ok(quote! {
                ::calimero_sdk::abi::TypeDef::Record {
                    doc: #doc,
                    fields: #fields,
                }
            })
        }
        Fields::Unnamed(unnamed) if unnamed.unnamed.len() > 1 => {
            let elements = tuple_elements(unnamed)?;
            Ok(quote! {
                ::calimero_sdk::abi::TypeDef::Alias {
                    doc: #doc,
                    target: ::calimero_sdk::abi::TypeRef::tuple(::std::vec![#(#elements),*]),
                    pattern: ::core::option::Option::None,
                }
            })
        }
        Fields::Unnamed(_) | Fields::Unit => Ok(quote! {
            ::calimero_sdk::abi::TypeDef::Record {
                doc: #doc,
                fields: ::std::vec![],
            }
        }),
    }
}

fn enum_def(
    enum_name: &str,
    data: &DataEnum,
    doc: Option<&str>,
    serde: &ContainerAttrs,
) -> Result<TokenStream, SynError> {
    let mut synthesized = Vec::new();
    let variants = wire_variants(enum_name, data.variants.iter(), serde, &mut synthesized)?
        .into_iter()
        .map(|WireVariant { name, payload, doc }| {
            quote! {
                ::calimero_sdk::abi::Variant {
                    name: #name.to_owned(),
                    code: ::core::option::Option::None,
                    payload: #payload,
                    doc: #doc,
                }
            }
        });

    let doc = doc::tokens(doc);
    let tag = option_string(serde.tag.as_ref().map(|(tag, _)| tag.as_str()));
    let content = option_string(serde.content.as_deref());
    let untagged = serde.untagged.is_some();
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

/// A variant as serde writes it: wire name, payload `TypeRef` expression, doc.
pub(crate) struct WireVariant {
    pub name: String,
    pub payload: TokenStream,
    pub doc: TokenStream,
}

/// The variants serde writes, in order, pushing the `define` call for every
/// synthesized payload record. Shared with `AbiEvents` so an event variant
/// describes identically.
pub(crate) fn wire_variants<'v>(
    enum_name: &str,
    variants: impl Iterator<Item = &'v syn::Variant>,
    serde: &ContainerAttrs,
    synthesized: &mut Vec<TokenStream>,
) -> Result<Vec<WireVariant>, SynError> {
    let mut out = Vec::new();
    for variant in variants {
        let attrs = serde_attrs::variant(&variant.attrs)?;
        if attrs.skip {
            continue;
        }
        let field_rule = attrs.rename_all.or(serde.rename_all_fields);
        out.push(WireVariant {
            name: serde.variant_name(&variant.ident, &attrs),
            payload: variant_payload(enum_name, variant, field_rule, synthesized)?,
            doc: doc::tokens(doc::doc_text(&variant.attrs).as_deref()),
        });
    }
    Ok(out)
}

/// The payload `TypeRef` expression for one variant, pushing the `define` call
/// for a synthesized `{Enum}_{Variant}` record when the shape needs one.
///
/// Both the emitted statements and the expression read a registry bound as
/// `__reg` at the call site.
fn variant_payload(
    enum_name: &str,
    variant: &syn::Variant,
    field_rule: Option<RenameRule>,
    synthesized: &mut Vec<TokenStream>,
) -> Result<TokenStream, SynError> {
    if variant.fields.is_empty() {
        return Ok(quote! { ::core::option::Option::None });
    }
    match &variant.fields {
        Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => {
            let field = &unnamed.unnamed[0];
            let ty = wire_type(field, &serde_attrs::field(&field.attrs)?)?;
            Ok(quote! {
                ::core::option::Option::Some(
                    <#ty as ::calimero_sdk::abi::AbiType>::type_ref(__reg)
                )
            })
        }
        Fields::Unnamed(unnamed) => {
            let elements = tuple_elements(unnamed)?;
            Ok(quote! {
                ::core::option::Option::Some(
                    ::calimero_sdk::abi::TypeRef::tuple(::std::vec![#(#elements),*])
                )
            })
        }
        Fields::Named(named) => {
            let record = format!("{}_{}", enum_name, variant.ident);
            let fields = fields_vec(named, field_rule)?;
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
        Fields::Unit => Ok(quote! { ::core::option::Option::None }),
    }
}

/// The `Field` list for a record; `Option` fields are nullable.
fn fields_vec(fields: &FieldsNamed, rule: Option<RenameRule>) -> Result<TokenStream, SynError> {
    let mut entries = Vec::new();
    for field in &fields.named {
        let serde = serde_attrs::field(&field.attrs)?;
        if serde.skip {
            continue;
        }
        let name = serde.rename.clone().unwrap_or_else(|| {
            let ident = field
                .ident
                .as_ref()
                .expect("a named field has an identifier")
                .unraw()
                .to_string();
            rule.map_or_else(|| ident.clone(), |rule| rule.apply_to_field(&ident))
        });
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

/// Each positional field's `type_ref` call; serde writes these as a JSON array.
fn tuple_elements(fields: &FieldsUnnamed) -> Result<Vec<TokenStream>, SynError> {
    let mut elements = Vec::new();
    for field in &fields.unnamed {
        let serde = serde_attrs::field(&field.attrs)?;
        if serde.skip {
            continue;
        }
        let ty = wire_type(field, &serde)?;
        elements.push(quote! { <#ty as ::calimero_sdk::abi::AbiType>::type_ref(__reg) });
    }
    Ok(elements)
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
    use super::*;

    fn rejection(input: DeriveInput) -> String {
        let serde = serde_attrs::container(&input.attrs).expect("container attrs parse");
        let described = match &input.data {
            Data::Struct(item) => struct_def(&item.fields, None, None, &serde),
            Data::Enum(item) => enum_def("E", item, None, &serde),
            Data::Union(_) => panic!("a union never reaches the describers"),
        };
        described.expect_err("the type must be refused").to_string()
    }

    #[test]
    fn an_untagged_struct_is_refused() {
        assert_eq!(
            rejection(parse_quote! {
                #[serde(untagged)]
                struct Untagged { id: u32 }
            }),
            ParseError::UnsupportedSerdeAttr {
                attr: "untagged".to_owned()
            }
            .to_string()
        );
    }

    #[test]
    fn a_custom_serialized_newtype_variant_needs_abi_as() {
        assert_eq!(
            rejection(parse_quote! {
                enum Id {
                    Hex(#[serde(serialize_with = "as_hex")] [u8; 2]),
                }
            }),
            ParseError::SerdeWireNeedsAbiAs {
                attr: "serialize_with".to_owned()
            }
            .to_string()
        );
    }

    #[test]
    fn a_field_abi_key_other_than_as_is_refused() {
        assert_eq!(
            rejection(parse_quote! {
                struct Blob {
                    #[abi(name = "x")]
                    id: u32,
                }
            }),
            "unsupported field `abi` key; expected `as = WireType`"
        );
    }
}
