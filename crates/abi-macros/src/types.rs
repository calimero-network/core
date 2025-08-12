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
use quote::ToTokens;
use syn::{parse::Parse, parse::ParseStream, Ident, Token, Type, TypePath, GenericArgument, PathArguments, TypeTuple, TypeArray, TypeSlice};

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
    Tuple(Vec<AbiTypeRef>),
    Array(Box<AbiTypeRef>, u32),
    Map(Box<AbiTypeRef>, Box<AbiTypeRef>, MapMode),
    Ref(String),
}

/// Map mode for Map types
#[derive(Debug, Clone)]
pub enum MapMode {
    Object,
    Entries,
}

impl AbiTypeRef {
    /// Convert Rust type to ABI type reference
    pub fn from_rust_type(ty: &Type) -> syn::Result<Self> {
        match ty {
            Type::Path(TypePath { path, .. }) => {
                if path.leading_colon.is_some() {
                    return Err(syn::Error::new_spanned(ty, "absolute paths not supported"));
                }
                
                if path.segments.len() != 1 {
                    return Err(syn::Error::new_spanned(ty, "complex paths not supported"));
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
                    "f32" | "f64" => {
                        Err(syn::Error::new_spanned(ty, "floating point types are not supported in ABI"))
                    }
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
                    "BTreeMap" | "HashMap" => {
                        if let PathArguments::AngleBracketed(args) = &segment.arguments {
                            if args.args.len() != 2 {
                                return Err(syn::Error::new_spanned(ty, "Map must have exactly two type parameters"));
                            }
                            
                            let key_arg = &args.args[0];
                            let value_arg = &args.args[1];
                            
                            if let (GenericArgument::Type(key_ty), GenericArgument::Type(value_ty)) = (key_arg, value_arg) {
                                let key_abi = Self::from_rust_type(key_ty)?;
                                let value_abi = Self::from_rust_type(value_ty)?;
                                
                                // Determine map mode based on key type
                                let mode = match &key_abi {
                                    AbiTypeRef::String => MapMode::Object,
                                    _ => MapMode::Entries,
                                };
                                
                                Ok(AbiTypeRef::Map(Box::new(key_abi), Box::new(value_abi), mode))
                            } else {
                                Err(syn::Error::new_spanned(ty, "Map type parameters must be types"))
                            }
                        } else {
                            Err(syn::Error::new_spanned(ty, "Map must have type parameters"))
                        }
                    }
                    _ => {
                        // For now, treat unknown types as references
                        Ok(AbiTypeRef::Ref(ident.to_string()))
                    }
                }
            }
            Type::Tuple(TypeTuple { elems, .. }) => {
                if elems.len() > 4 {
                    return Err(syn::Error::new_spanned(ty, "tuples with more than 4 elements are not supported"));
                }
                
                let mut items = Vec::new();
                for elem in elems {
                    let item_abi = Self::from_rust_type(elem)?;
                    items.push(item_abi);
                }
                
                Ok(AbiTypeRef::Tuple(items))
            }
            Type::Array(TypeArray { elem, len, .. }) => {
                let elem_abi = Self::from_rust_type(elem)?;
                
                // Parse the length expression
                let len_expr = syn::parse2::<syn::Expr>(len.into_token_stream())?;
                let len_value = match len_expr {
                    syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(lit), .. }) => {
                        lit.base10_parse::<u32>()?
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(len, "array length must be a literal integer"));
                    }
                };
                
                Ok(AbiTypeRef::Array(Box::new(elem_abi), len_value))
            }
            _ => Err(syn::Error::new_spanned(ty, "unsupported type")),
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
            AbiTypeRef::Tuple(items) => {
                let items_json: Vec<serde_json::Value> = items.iter().map(|item| item.to_json()).collect();
                serde_json::json!({
                    "type": "tuple",
                    "items": items_json
                })
            }
            AbiTypeRef::Array(item, len) => {
                serde_json::json!({
                    "type": "array",
                    "value": item.to_json(),
                    "len": len
                })
            }
            AbiTypeRef::Map(key, value, mode) => {
                let mode_str = match mode {
                    MapMode::Object => "object",
                    MapMode::Entries => "entries",
                };
                serde_json::json!({
                    "type": "map",
                    "key": key.to_json(),
                    "value": value.to_json(),
                    "mode": mode_str
                })
            }
            AbiTypeRef::Ref(name) => {
                serde_json::json!({
                    "$ref": name
                })
            }
        }
    }
} 