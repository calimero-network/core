use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::{Error as SynError, Ident, Result as SynResult, Token};

use crate::errors::{Errors, ParseError};
use crate::items::StructOrEnumItem;
use crate::reserved::idents;

pub struct PrivateImpl<'a> {
    ident: &'a Ident,
    key_name: String,
    key_bytes: Vec<u8>,
    orig: &'a StructOrEnumItem,
}

impl ToTokens for PrivateImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let PrivateImpl {
            ident,
            key_name,
            key_bytes,
            orig,
        } = self;

        let key_bytes_literal = proc_macro2::Literal::byte_string(key_bytes);
        let key_name_ident = Ident::new(key_name, ident.span());

        quote! {
            #orig

            // Define the key constant
            const #key_name_ident: &[u8] = #key_bytes_literal;

            impl #ident {
                /// Get a handle to the private storage for this type
                pub fn private_handle() -> ::calimero_sdk::private_storage::EntryHandle<Self> {
                    ::calimero_sdk::private_storage::EntryHandle::new(#key_name_ident)
                }

                /// Load the state from private storage
                pub fn private_load() -> ::calimero_sdk::app::Result<Option<::calimero_sdk::private_storage::EntryRef<Self>>> {
                    Self::private_handle().get()
                }

                /// Load the state or initialize with default
                pub fn private_load_or_default() -> ::calimero_sdk::app::Result<::calimero_sdk::private_storage::EntryRef<Self>>
                where
                    Self: Default,
                {
                    Self::private_handle().get_or_default()
                }

                /// Load the state or initialize with a custom function
                pub fn private_load_or_init_with<F>(f: F) -> ::calimero_sdk::app::Result<::calimero_sdk::private_storage::EntryRef<Self>>
                where
                    F: FnOnce() -> Self,
                {
                    Self::private_handle().get_or_init_with(f)
                }
            }
        }
        .to_tokens(tokens);
    }
}

pub struct PrivateArgs {
    key: Option<Vec<u8>>,
}

impl Parse for PrivateArgs {
    fn parse(input: ParseStream<'_>) -> SynResult<Self> {
        let mut key = None;

        if !input.is_empty() {
            if !input.peek(Ident) {
                return Err(input.error("expected an identifier"));
            }

            let ident = input.parse::<Ident>()?;

            if !input.peek(Token![=]) {
                let span = if let Some((tt, _)) = input.cursor().token_tree() {
                    tt.span()
                } else {
                    ident.span()
                };
                return Err(SynError::new(
                    span,
                    format_args!("expected `=` after `{ident}`"),
                ));
            }

            let eq = input.parse::<Token![=]>()?;

            match ident.to_string().as_str() {
                "key" => {
                    if input.is_empty() {
                        return Err(SynError::new_spanned(
                            eq,
                            "expected a byte string after `=`",
                        ));
                    }

                    // Parse a byte string literal
                    let key_lit = input.parse::<syn::LitByteStr>()?;
                    key = Some(key_lit.value());
                }
                _ => {
                    return Err(SynError::new_spanned(
                        &ident,
                        format_args!("unexpected `{ident}`"),
                    ));
                }
            }

            if !input.is_empty() {
                return Err(input.error("unexpected token"));
            }
        }

        Ok(Self { key })
    }
}

pub struct PrivateImplInput<'a> {
    pub item: &'a StructOrEnumItem,
    pub args: &'a PrivateArgs,
}

impl<'a> TryFrom<PrivateImplInput<'a>> for PrivateImpl<'a> {
    type Error = Errors<'a, StructOrEnumItem>;

    fn try_from(input: PrivateImplInput<'a>) -> Result<Self, Self::Error> {
        let errors = Errors::new(input.item);

        let (ident, _generics) = match input.item {
            StructOrEnumItem::Struct(item) => (&item.ident, &item.generics),
            StructOrEnumItem::Enum(item) => (&item.ident, &item.generics),
        };

        if ident == &*idents::input() {
            errors.subsume(SynError::new_spanned(ident, ParseError::UseOfReservedIdent));
        }

        // Generate a default key if none provided
        let key_bytes = input.args.key.clone().unwrap_or_else(|| {
            let mut key = ident.to_string().as_bytes().to_vec();
            key.truncate(32); // Ensure it fits in our key storage
            key
        });

        // Generate key name
        let key_name = format!("{}_KEY", ident.to_string().to_uppercase());

        errors.check()?;

        Ok(PrivateImpl {
            ident,
            key_name,
            key_bytes,
            orig: input.item,
        })
    }
}
