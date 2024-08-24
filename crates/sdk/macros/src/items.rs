use proc_macro2::TokenStream;
use quote::ToTokens;
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, ItemEnum, ItemStruct, Result as SynResult, Token, Visibility};

pub enum StructOrEnumItem {
    Struct(ItemStruct),
    Enum(ItemEnum),
}

impl Parse for StructOrEnumItem {
    fn parse(input: ParseStream<'_>) -> SynResult<Self> {
        let mut attrs = Vec::new();
        let mut vis = Visibility::Inherited;
        let item = loop {
            let lookahead = input.lookahead1();
            if lookahead.peek(Token![struct]) {
                let mut struct_: ItemStruct = input.parse()?;
                struct_.attrs = attrs;
                struct_.vis = vis;
                break Self::Struct(struct_);
            } else if lookahead.peek(Token![enum]) {
                let mut enum_: ItemEnum = input.parse()?;
                enum_.attrs = attrs;
                enum_.vis = vis;
                break Self::Enum(enum_);
            } else if lookahead.peek(Token![#]) {
                attrs.extend(input.call(Attribute::parse_outer)?);
            } else if lookahead.peek(Token![pub]) {
                vis = input.parse::<Visibility>()?;
            } else {
                return Err(lookahead.error());
            }
        };

        input
            .is_empty()
            .then_some(item)
            .ok_or_else(|| input.error("unexpected token"))
    }
}

impl ToTokens for StructOrEnumItem {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        match self {
            Self::Struct(item) => item.to_tokens(tokens),
            Self::Enum(item) => item.to_tokens(tokens),
        }
    }
}

pub struct Empty {
    _priv: (),
}

impl Parse for Empty {
    fn parse(input: ParseStream<'_>) -> SynResult<Self> {
        if input.is_empty() {
            return Ok(Self { _priv: () });
        }

        Err(input.error("unexpected token"))
    }
}
