use proc_macro2::TokenStream;
use quote::{format_ident, quote, ToTokens};
use syn::{parse_quote, Data, Error as SynError, Fields, GenericParam, Generics, Ident, Visibility};

use crate::errors::{Errors, ParseError};
use crate::items::StructOrEnumItem;
use crate::reserved::{idents, lifetimes};

// Helper function to convert PascalCase to snake_case
fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    
    while let Some(c) = chars.next() {
        if c.is_uppercase() && !result.is_empty() {
            result.push('_');
        }
        result.push(c.to_lowercase().next().unwrap_or(c));
    }
    
    result
}

pub struct EventImpl<'a> {
    ident: &'a Ident,
    generics: &'a Generics,
    orig: &'a StructOrEnumItem,
}

impl ToTokens for EventImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let EventImpl {
            ident,
            generics: source_generics,
            orig,
        } = *self;

        let mut generics = source_generics.clone();

        for generic_ty in source_generics.type_params() {
            generics
                .make_where_clause()
                .predicates
                .push(parse_quote!(#generic_ty: ::calimero_sdk::serde::Serialize));
        }

        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        // Parse the event enum variants and generate handler methods at compile time
        let mut handler_methods = Vec::new();
        
        if let StructOrEnumItem::Enum(enum_item) = orig {
            for variant in &enum_item.variants {
                let v_ident = &variant.ident;
                let handler_name = format_ident!("on_{}", to_snake_case(&v_ident.to_string()));
                
                // Generate handler method based on variant fields
                match &variant.fields {
                    Fields::Named(fields) => {
                        let field_names: Vec<_> = fields.named.iter().map(|f| &f.ident).collect();
                        let field_types: Vec<_> = fields.named.iter().map(|f| &f.ty).collect();
                        
                        handler_methods.push(quote! {
                            fn #handler_name(&mut self, #(#field_names: #field_types),*) -> ::calimero_sdk::app::Result<()> {
                                Ok(())
                            }
                        });
                    }
                    Fields::Unnamed(fields) => {
                        let field_types: Vec<_> = fields.unnamed.iter().map(|f| &f.ty).collect();
                        let field_names: Vec<_> = (0..field_types.len()).map(|i| format_ident!("arg{}", i)).collect();
                        
                        handler_methods.push(quote! {
                            fn #handler_name(&mut self, #(#field_names: #field_types),*) -> ::calimero_sdk::app::Result<()> {
                                Ok(())
                            }
                        });
                    }
                    Fields::Unit => {
                        handler_methods.push(quote! {
                            fn #handler_name(&mut self) -> ::calimero_sdk::app::Result<()> {
                                Ok(())
                            }
                        });
                    }
                }
            }
        }

        quote! {
            #[derive(::calimero_sdk::serde::Serialize, ::calimero_sdk::serde::Deserialize, ::calimero_sdk::CallbackHandlers)]
            #[serde(crate = "::calimero_sdk::serde")]
            #[serde(tag = "kind", content = "data")]
            #orig

            impl #impl_generics ::calimero_sdk::event::AppEvent for #ident #ty_generics #where_clause {
                fn kind(&self) -> ::std::borrow::Cow<str> {
                    // todo! revisit quick
                    match ::calimero_sdk::serde_json::to_value(self) {
                        Ok(data) => ::std::borrow::Cow::Owned(data["kind"].as_str().expect("Failed to get event kind").to_string()),
                        Err(err) => ::calimero_sdk::env::panic_str(
                            &format!("Failed to serialize event: {:?}", err)
                        ),
                    }
                }
                fn data(&self) -> ::std::borrow::Cow<[u8]> {
                    // todo! revisit quick
                    match ::calimero_sdk::serde_json::to_value(self) {
                        Ok(data) => ::std::borrow::Cow::Owned(::calimero_sdk::serde_json::to_vec(&data["data"]).expect("Failed to serialize event data")),
                        Err(err) => ::calimero_sdk::env::panic_str(
                            &format!("Failed to serialize event: {:?}", err)
                        ),
                    }
                }
            }

            impl #impl_generics ::calimero_sdk::event::AppEventExt for #ident #ty_generics #where_clause {}

            // Generate a macro that can be used by #[app::logic] to generate handler methods
            #[macro_export]
            macro_rules! #ident {
                ($app_type:ty) => {
                    #(#handler_methods)*
                };
            }
        }
        .to_tokens(tokens);
    }
}

pub struct EventImplInput<'a> {
    pub item: &'a StructOrEnumItem,
}

impl<'a> TryFrom<EventImplInput<'a>> for EventImpl<'a> {
    type Error = Errors<'a, StructOrEnumItem>;

    fn try_from(input: EventImplInput<'a>) -> Result<Self, Self::Error> {
        let errors = Errors::new(input.item);

        let (vis, ident, generics) = match input.item {
            StructOrEnumItem::Struct(item) => (&item.vis, &item.ident, &item.generics),
            StructOrEnumItem::Enum(item) => (&item.vis, &item.ident, &item.generics),
        };

        match vis {
            Visibility::Public(_) => {}
            Visibility::Inherited => {
                return Err(errors.finish(SynError::new_spanned(ident, ParseError::NoPrivateEvent)));
            }
            Visibility::Restricted(spec) => {
                return Err(
                    errors.finish(SynError::new_spanned(spec, ParseError::NoComplexVisibility))
                );
            }
        }

        if ident == &*idents::input() {
            errors.subsume(SynError::new_spanned(ident, ParseError::UseOfReservedIdent));
        }

        for generic in &generics.params {
            match generic {
                GenericParam::Lifetime(params) => {
                    if params.lifetime == *lifetimes::input() {
                        errors.subsume(SynError::new(
                            params.lifetime.span(),
                            ParseError::UseOfReservedLifetime,
                        ));
                    }
                }
                GenericParam::Type(params) => {
                    if params.ident == *idents::input() {
                        errors.subsume(SynError::new_spanned(
                            &params.ident,
                            ParseError::UseOfReservedIdent,
                        ));
                    }
                }
                GenericParam::Const(_) => {}
            }
        }

        errors.check()?;

        Ok(EventImpl {
            ident,
            generics,
            orig: input.item,
        })
    }
}
