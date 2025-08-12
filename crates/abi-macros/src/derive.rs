// Copyright 2024 Calimero Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Data, Fields, Variant, Type};

pub fn derive_abi_type_impl(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    
    // For now, just generate a placeholder implementation
    // The actual type analysis will be done during ABI generation
    let expanded = quote! {
        impl AbiType for #name {
            fn abi_type() -> &'static str {
                stringify!(#name)
            }
        }
    };
    
    expanded.into()
}

/// Analyze a struct or enum to extract its ABI type information
pub fn analyze_abi_type(input: &DeriveInput) -> syn::Result<AbiTypeInfo> {
    let name = &input.ident;
    
    match &input.data {
        Data::Struct(data) => {
            let fields = analyze_struct_fields(&data.fields)?;
            Ok(AbiTypeInfo::Struct {
                name: name.to_string(),
                fields,
                newtype: is_newtype_struct(&data.fields),
            })
        }
        Data::Enum(data) => {
            let variants = analyze_enum_variants(&data.variants.iter().collect::<Vec<_>>())?;
            Ok(AbiTypeInfo::Enum {
                name: name.to_string(),
                variants,
            })
        }
        Data::Union(_) => {
            Err(syn::Error::new_spanned(name, "unions are not supported in ABI"))
        }
    }
}

/// ABI type information extracted from a Rust type
#[derive(Debug, Clone)]
pub enum AbiTypeInfo {
    Struct {
        name: String,
        fields: Vec<FieldInfo>,
        newtype: bool,
    },
    Enum {
        name: String,
        variants: Vec<VariantInfo>,
    },
}

/// Field information for structs
#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: String,
    pub ty: String,
}

/// Variant information for enums
#[derive(Debug, Clone)]
pub struct VariantInfo {
    pub name: String,
    pub kind: VariantKindInfo,
}

/// Variant kind information
#[derive(Debug, Clone)]
pub enum VariantKindInfo {
    Unit,
    Tuple(Vec<String>),
    Struct(Vec<FieldInfo>),
}

fn analyze_struct_fields(fields: &Fields) -> syn::Result<Vec<FieldInfo>> {
    let mut field_infos = Vec::new();
    
    for field in fields.iter() {
        let field_name = field.ident.as_ref()
            .ok_or_else(|| syn::Error::new_spanned(field, "unnamed fields not supported"))?
            .to_string();
        
        let field_type = type_to_string(&field.ty)?;
        
        field_infos.push(FieldInfo {
            name: field_name,
            ty: field_type,
        });
    }
    
    Ok(field_infos)
}

fn analyze_enum_variants(variants: &[&Variant]) -> syn::Result<Vec<VariantInfo>> {
    let mut variant_infos = Vec::new();
    
    for variant in variants {
        let variant_name = variant.ident.to_string();
        
        let variant_kind = match &variant.fields {
            Fields::Unit => VariantKindInfo::Unit,
            Fields::Unnamed(fields) => {
                let mut types = Vec::new();
                for field in &fields.unnamed {
                    let field_type = type_to_string(&field.ty)?;
                    types.push(field_type);
                }
                VariantKindInfo::Tuple(types)
            }
            Fields::Named(fields) => {
                let mut field_infos = Vec::new();
                for field in &fields.named {
                    let field_name = field.ident.as_ref()
                        .ok_or_else(|| syn::Error::new_spanned(field, "unnamed fields not supported"))?
                        .to_string();
                    
                    let field_type = type_to_string(&field.ty)?;
                    
                    field_infos.push(FieldInfo {
                        name: field_name,
                        ty: field_type,
                    });
                }
                VariantKindInfo::Struct(field_infos)
            }
        };
        
        variant_infos.push(VariantInfo {
            name: variant_name,
            kind: variant_kind,
        });
    }
    
    Ok(variant_infos)
}

fn type_to_string(ty: &Type) -> syn::Result<String> {
    match ty {
        Type::Path(type_path) => {
            let path = &type_path.path;
            if path.segments.len() == 1 {
                Ok(path.segments[0].ident.to_string())
            } else {
                // For complex paths, just use the last segment for now
                Ok(path.segments.last().unwrap().ident.to_string())
            }
        }
        Type::Tuple(tuple) => {
            let mut types = Vec::new();
            for elem in &tuple.elems {
                types.push(type_to_string(elem)?);
            }
            Ok(format!("({})", types.join(", ")))
        }
        Type::Array(array) => {
            let elem_type = type_to_string(&array.elem)?;
            // Note: array length is not included in the string representation
            Ok(format!("[{}; N]", elem_type))
        }
        Type::Slice(slice) => {
            let elem_type = type_to_string(&slice.elem)?;
            Ok(format!("[{}]", elem_type))
        }
        _ => Err(syn::Error::new_spanned(ty, "unsupported type in ABI analysis")),
    }
}

fn is_newtype_struct(fields: &Fields) -> bool {
    matches!(fields, Fields::Unnamed(fields) if fields.unnamed.len() == 1)
} 