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

use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse::Parse, parse::ParseStream, Ident, Token, Type, TypePath, GenericArgument, PathArguments};

/// ABI type reference for macro generation
#[derive(Debug, Clone)]
pub enum AbiTypeRef {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    U128,
    I128,
    String,
    Bytes,
    Option(Box<AbiTypeRef>),
    Vec(Box<AbiTypeRef>),
    Ref(String),
}

impl AbiTypeRef {
    /// Convert Rust type to ABI type reference
    pub fn from_rust_type(ty: &Type) -> syn::Result<Self> {
        match ty {
            Type::Path(TypePath { path, .. }) => {
                if path.leading_colon.is_some() {
                    return Err(syn::Error::new_spanned(ty, "absolute paths not supported in PR1"));
                }
                
                if path.segments.len() != 1 {
                    return Err(syn::Error::new_spanned(ty, "complex paths not supported in PR1"));
                }
                
                let segment = &path.segments[0];
                let ident = &segment.ident;
                
                match ident.to_string().as_str() {
                    "bool" => Ok(AbiTypeRef::Bool),
                    "u8" => Ok(AbiTypeRef::U8),
                    "u16" => Ok(AbiTypeRef::U16),
                    "u32" => Ok(AbiTypeRef::U32),
                    "u64" => Ok(AbiTypeRef::U64),
                    "i8" => Ok(AbiTypeRef::I8),
                    "i16" => Ok(AbiTypeRef::I16),
                    "i32" => Ok(AbiTypeRef::I32),
                    "i64" => Ok(AbiTypeRef::I64),
                    "u128" => Ok(AbiTypeRef::U128),
                    "i128" => Ok(AbiTypeRef::I128),
                    "String" => Ok(AbiTypeRef::String),
                    "Vec" => {
                        if let PathArguments::AngleBracketed(args) = &segment.arguments {
                            if args.args.len() != 1 {
                                return Err(syn::Error::new_spanned(ty, "Vec must have exactly one type parameter"));
                            }
                            
                            let arg = &args.args[0];
                            if let GenericArgument::Type(inner_ty) = arg {
                                let inner_abi = Self::from_rust_type(inner_ty)?;
                                Ok(AbiTypeRef::Vec(Box::new(inner_abi)))
                            } else {
                                Err(syn::Error::new_spanned(ty, "Vec type parameter must be a type"))
                            }
                        } else {
                            Err(syn::Error::new_spanned(ty, "Vec must have type parameters"))
                        }
                    }
                    "Option" => {
                        if let PathArguments::AngleBracketed(args) = &segment.arguments {
                            if args.args.len() != 1 {
                                return Err(syn::Error::new_spanned(ty, "Option must have exactly one type parameter"));
                            }
                            
                            let arg = &args.args[0];
                            if let GenericArgument::Type(inner_ty) = arg {
                                let inner_abi = Self::from_rust_type(inner_ty)?;
                                Ok(AbiTypeRef::Option(Box::new(inner_abi)))
                            } else {
                                Err(syn::Error::new_spanned(ty, "Option type parameter must be a type"))
                            }
                        } else {
                            Err(syn::Error::new_spanned(ty, "Option must have type parameters"))
                        }
                    }
                    _ => {
                        // For now, treat unknown types as references
                        Ok(AbiTypeRef::Ref(ident.to_string()))
                    }
                }
            }
            _ => Err(syn::Error::new_spanned(ty, "unsupported type in PR1")),
        }
    }
    
    /// Convert to JSON representation
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            AbiTypeRef::Bool => serde_json::json!({"type": "bool"}),
            AbiTypeRef::U8 => serde_json::json!({"type": "u8"}),
            AbiTypeRef::U16 => serde_json::json!({"type": "u16"}),
            AbiTypeRef::U32 => serde_json::json!({"type": "u32"}),
            AbiTypeRef::U64 => serde_json::json!({"type": "u64"}),
            AbiTypeRef::I8 => serde_json::json!({"type": "i8"}),
            AbiTypeRef::I16 => serde_json::json!({"type": "i16"}),
            AbiTypeRef::I32 => serde_json::json!({"type": "i32"}),
            AbiTypeRef::I64 => serde_json::json!({"type": "i64"}),
            AbiTypeRef::U128 => serde_json::json!({"type": "u128"}),
            AbiTypeRef::I128 => serde_json::json!({"type": "i128"}),
            AbiTypeRef::String => serde_json::json!({"type": "string"}),
            AbiTypeRef::Bytes => serde_json::json!({"type": "bytes"}),
            AbiTypeRef::Option(inner) => {
                serde_json::json!({
                    "type": "option",
                    "value": inner.to_json()
                })
            }
            AbiTypeRef::Vec(inner) => {
                serde_json::json!({
                    "type": "vec",
                    "value": inner.to_json()
                })
            }
            AbiTypeRef::Ref(name) => {
                serde_json::json!({
                    "type": "ref",
                    "value": name
                })
            }
        }
    }
} 