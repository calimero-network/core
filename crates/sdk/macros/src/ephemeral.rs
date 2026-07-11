use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::Error as SynError;

use crate::errors::{Errors, ParseError};
use crate::items::StructOrEnumItem;

pub struct EphemeralImpl<'a> {
    orig: &'a StructOrEnumItem,
}

impl ToTokens for EphemeralImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let orig = self.orig;

        quote! {
            #[derive(
                ::calimero_sdk::borsh::BorshSerialize,
                ::calimero_sdk::borsh::BorshDeserialize,
                ::calimero_sdk::serde::Serialize,
                ::calimero_sdk::serde::Deserialize,
            )]
            #[borsh(crate = "::calimero_sdk::borsh")]
            #[serde(crate = "::calimero_sdk::serde")]
            #orig
        }
        .to_tokens(tokens);
    }
}

pub struct EphemeralImplInput<'a> {
    pub item: &'a StructOrEnumItem,
}

impl<'a> TryFrom<EphemeralImplInput<'a>> for EphemeralImpl<'a> {
    type Error = Errors<'a, StructOrEnumItem>;

    fn try_from(input: EphemeralImplInput<'a>) -> Result<Self, Self::Error> {
        let errors = Errors::new(input.item);

        match input.item {
            StructOrEnumItem::Struct(_) => {}
            StructOrEnumItem::Enum(item) => {
                return Err(errors.finish(SynError::new_spanned(
                    &item.ident,
                    ParseError::EphemeralMustBeStruct,
                )));
            }
        }

        errors.check()?;

        Ok(EphemeralImpl { orig: input.item })
    }
}
