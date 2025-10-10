use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Ident, Type, TypePath, TypeReference};

pub fn derive_callback_handlers(input: DeriveInput) -> TokenStream2 {
    let enum_ident = input.ident;
    let generics = input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    // Use the first declared lifetime (if any) to tie deserialized borrows to the input bytes
    let data_ty: TokenStream2 = if let Some(lif) = generics.lifetimes().next() {
        let lt = &lif.lifetime;
        quote! { &#lt [u8] }
    } else {
        quote! { &[u8] }
    };

    let Data::Enum(data_enum) = input.data else {
        return quote! {
            compile_error!("CallbackHandlers can only be derived for enums");
        };
    };

    // Generate trait methods and match arms from enum variants
    let mut trait_methods = Vec::new();
    let mut match_arms = Vec::new();

    for variant in data_enum.variants {
        let v_ident = variant.ident;
        let handler_name = format_ident!("on_{}", to_snake(&v_ident));

        match variant.fields {
            Fields::Named(named) => {
                let mut params = Vec::new();
                let mut binds = Vec::new();
                let mut call_args = Vec::new();
                let mut param_types = Vec::new();

                for field in named.named {
                    let fname: Ident = field.ident.expect("named field");
                    let (pty, is_string_param) = param_type_for(&field.ty);
                    params.push(quote! { #fname: #pty });
                    binds.push(quote! { #fname });
                    param_types.push(pty);
                    if is_string_param {
                        call_args.push(quote! { #fname.to_string() });
                    } else {
                        call_args.push(quote! { #fname });
                    }
                }

                trait_methods.push(quote! {
                    fn #handler_name(&mut self, #( #params ),*) -> ::calimero_sdk::app::Result<()> { Ok(()) }
                });

                match_arms.push(quote! {
                    stringify!(#v_ident) => {
                        // Deserialize the event data directly - it's already the inner data object
                        let json_value: ::calimero_sdk::serde_json::Value = ::calimero_sdk::serde_json::from_slice(data)
                            .map_err(|_| ::calimero_sdk::types::Error::msg("event decode failed"))?;

                        // Extract fields from JSON - the data is already the inner object
                        #(
                            let #binds = ::calimero_sdk::serde_json::from_value(
                                json_value.get(stringify!(#binds))
                                    .ok_or_else(|| ::calimero_sdk::types::Error::msg("missing field"))?
                                    .clone()
                            ).map_err(|_| ::calimero_sdk::types::Error::msg("field decode failed"))?;
                        )*
                        target.#handler_name( #( #binds ),* )
                    }
                });
            }
            Fields::Unnamed(_) | Fields::Unit => {
                // For tuple/unit variants, pass no fields
                trait_methods.push(quote! {
                    fn #handler_name(&mut self) -> ::calimero_sdk::app::Result<()> { Ok(()) }
                });

                match_arms.push(quote! {
                    stringify!(#v_ident) => {
                        let _v: Self = ::calimero_sdk::serde_json::from_slice(data)
                            .map_err(|_| ::calimero_sdk::types::Error::msg("event decode failed"))?;
                        target.#handler_name()
                    }
                });
            }
        }
    }

    quote! {
        pub trait CallbackHandlers {
            #( #trait_methods )*
        }

        impl #impl_generics #enum_ident #ty_generics #where_clause {
            pub fn dispatch<T: CallbackHandlers>(
                target: &mut T,
                kind: &str,
                data: #data_ty,
            ) -> ::calimero_sdk::app::Result<()> {
                match kind {
                    #( #match_arms )*
                    _ => Ok(())
                }
            }
        }
    }
}

fn param_type_for(ty: &Type) -> (TokenStream2, bool) {
    match ty {
        Type::Reference(TypeReference { elem, .. }) => match &**elem {
            Type::Path(tp) if is_str_path(tp) => (quote! { ::std::string::String }, true),
            _ => (quote! { ::std::string::String }, true), // default treat as string-ish
        },
        Type::Path(tp) if is_str_path(tp) => (quote! { ::std::string::String }, true),
        Type::Path(tp) if is_u64_path(tp) => (quote! { u64 }, false),
        _ => (quote! { ::std::string::String }, true),
    }
}

fn is_str_path(tp: &TypePath) -> bool {
    tp.path
        .segments
        .last()
        .map(|s| s.ident == "str")
        .unwrap_or(false)
}

fn is_u64_path(tp: &TypePath) -> bool {
    tp.path
        .segments
        .last()
        .map(|s| s.ident == "u64")
        .unwrap_or(false)
}

fn to_snake(ident: &Ident) -> String {
    let s = ident.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i != 0 {
                out.push('_');
            }
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
        } else {
            out.push(ch);
        }
    }
    out
}
