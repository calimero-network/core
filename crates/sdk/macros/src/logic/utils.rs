use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;

use crate::errors;

/// Returns `T` in `T`, `(T)` and an invisible group with T if `!deref`.
///
/// Also returns T in `&T`, `&mut T`, `(&T)`, `(&mut T)`, `(&T,)`, `(&mut T,)` and groups if `deref`
pub fn typed_path(ty: &syn::Type, deref: bool) -> Option<&syn::Path> {
    #[cfg_attr(all(test, feature = "nightly"), deny(non_exhaustive_omitted_patterns))]
    match ty {
        syn::Type::Path(path) => Some(&path.path),
        syn::Type::Reference(reference) if deref => typed_path(&reference.elem, deref),
        syn::Type::Group(group) => typed_path(&group.elem, deref),
        syn::Type::Paren(paren) => typed_path(&paren.elem, deref),
        _ => None,
    }
}

fn _sanitize_self<'a>(
    ty: impl Iterator<Item = TokenTree> + 'a,
    replace_with: &'a TokenStream,
) -> impl Iterator<Item = TokenTree> + 'a {
    ty.into_iter().flat_map(|tt| match tt {
        TokenTree::Ident(ident) if ident == "Self" => {
            Box::new(replace_with.clone().into_iter().map(move |mut tree| {
                tree.set_span(ident.span());
                tree
            }))
        }
        TokenTree::Group(group) => {
            Box::new(std::iter::once(TokenTree::Group(proc_macro2::Group::new(
                group.delimiter(),
                _sanitize_self(group.stream().into_iter(), replace_with).collect(),
            )))) as Box<dyn Iterator<Item = _>>
        }
        _ => Box::new(std::iter::once(tt)),
    })
}

pub fn sanitize_self<'a>(
    ty: &syn::Type,
    replace_with: &'a TokenStream,
) -> impl Iterator<Item = TokenTree> + 'a {
    _sanitize_self(ty.to_token_stream().into_iter(), replace_with)
}

// NAHHHHH. Fuck you!
// pub fn _sanitize_lifetime<'a>(
//     ty: impl Iterator<Item = TokenTree> + 'a,
//     replace_with: &'a TokenStream,
// ) -> impl Iterator<Item = TokenTree> + 'a {
//     enum State {
//         Ref(proc_macro2::Punct),
//         Tick(proc_macro2::Punct),
//     }

//     let mut state = None;
//     ty.into_iter().flat_map(move |tt| match (tt, state.take()) {
//         // &
//         (TokenTree::Punct(punct), None)
//             if punct.as_char() == '&' && punct.spacing() == proc_macro2::Spacing::Joint =>
//         {
//             state = Some(State::Ref(punct));
//             Box::new(std::iter::empty())
//         }
//         (TokenTree::Punct(punct), old_state)
//             if punct.as_char() == '\'' && punct.spacing() == proc_macro2::Spacing::Joint =>
//         {
//             match old_state {
//                 Some(State::Ref(ref_)) => {
//                     state = Some(State::Tick(punct));
//                     Box::new(std::iter::once(TokenTree::Punct(ref_)))
//                 }
//                 Some(State::Tick(old_tick)) => {
//                     state = None;
//                     let old_tick = std::iter::once(TokenTree::Punct(old_tick));
//                     let new_tick = std::iter::once(TokenTree::Punct(punct));
//                     Box::new(old_tick.chain(new_tick)) as Box<dyn Iterator<Item = _>>
//                 }
//                 None => {
//                     state = Some(State::Tick(punct));
//                     Box::new(std::iter::empty())
//                 }
//             }
//         }
//         // (TokenTree::Ident(ident), Some(last_ref), None) => Box::new(
//         //     // std::iter::once(TokenTree::Punct(punct))
//         //     //     .chain(std::iter::once(TokenTree::Ident(ident))),
//         // ),
//         // (TokenTree::Ident(ident), None, Some(punct)) => Box::new(
//         //     // std::iter::once(TokenTree::Punct(punct))
//         //     //     .chain(std::iter::once(TokenTree::Ident(ident))),
//         // ),
//         // TokenTree::Group(group) => {
//         //     Box::new(std::iter::once(TokenTree::Group(proc_macro2::Group::new(
//         //         group.delimiter(),
//         //         _sanitize_lifetime(group.stream().into_iter(), replace_with).collect(),
//         //     )))) as Box<dyn Iterator<Item = _>>
//         // }
//         (tt, _) => Box::new(std::iter::once(tt)),
//     })
// }

// pub fn __sanitize_self(ty: &mut syn::Type, replace_with: &syn::Path) {
//     #[cfg_attr(all(test, feature = "nightly"), deny(non_exhaustive_omitted_patterns))]
//     match ty {
//         syn::Type::Reference(reference) => sanitize_self(&mut reference.elem, replace_with),
//         syn::Type::Group(group) => sanitize_self(&mut group.elem, replace_with),
//         syn::Type::Paren(paren) => sanitize_self(&mut paren.elem, replace_with),
//         syn::Type::Path(path) => {
//             if let Some(qpath) = &mut path.qself {
//                 sanitize_self(&mut qpath.ty, replace_with);
//             }
//             // path.path.segments = std::mem::take(&mut path.path.segments)
//             //     .into_iter()
//             //     .flat_map(|seg| {
//             //         if seg.ident == "Self" {
//             //             Box::new(replace_with.segments.iter().cloned())
//             //                 as Box<dyn Iterator<Item = _>>
//             //         } else {
//             //             Box::new(std::iter::once(seg)) as Box<_>
//             //         }
//             //     })
//             //     .collect();
//             // path.path.segments = path
//             //     .path
//             //     .segments
//             //     .iter()
//             //     .flat_map(|seg| {
//             //         if seg.ident == "Self" {
//             //             replace_with.segments.iter().cloned()
//             //         } else {
//             //             std::iter::once(seg.clone())
//             //         }
//             //     })
//             // for seg in path.path.segments.iter_mut() {
//             //     if seg.ident == "Self" {
//             //         seg.ident = replace_with.segments.last().unwrap().ident.clone();
//             //     }
//             // }
//             // if path.path.is_ident("Self") {
//             //     *path = syn::TypePath {
//             //         qself: None,
//             //         path: replace_with.clone(),
//             //     };
//             // }
//         }
//         syn::Type::Array(array) => sanitize_self(&mut array.elem, replace_with),
//         syn::Type::Slice(slice) => sanitize_self(&mut slice.elem, replace_with),
//         syn::Type::Tuple(tuple) => {
//             for elem in tuple.elems.iter_mut() {
//                 sanitize_self(elem, replace_with);
//             }
//         }
//         syn::Type::BareFn(fn_) => {
//             for arg in fn_.inputs.iter_mut() {
//                 sanitize_self(&mut arg.ty, replace_with);
//             }
//             if let syn::ReturnType::Type(_, ty) = &mut fn_.output {
//                 sanitize_self(ty, replace_with);
//             }
//         }
//         syn::Type::ImplTrait(impl_) => {
//             for bound in impl_.bounds.iter_mut() {
//                 match bound {
//                     syn::TypeParamBound::Trait(trait_) => {
//                         for path in trait_.path.segments.iter_mut() {
//                             if path.ident == "Self" {
//                                 path.ident = replace_with.segments.last().unwrap().ident.clone();
//                             }
//                         }
//                     }
//                     _ => {}
//                 }
//             }
//         }
//         syn::Type::Infer(_) => todo!(),
//         syn::Type::Macro(_) => todo!(),
//         syn::Type::Never(_) => todo!(),
//         syn::Type::Ptr(_) => todo!(),
//         syn::Type::TraitObject(_) => todo!(),
//         syn::Type::Verbatim(_) => todo!(),
//         _ => todo!(),
//     }
// }

// note! we won't be able to reconstruct the types if we permit nested pattern definitions
// note! oops
pub fn collect_idents_in_pattern<'a, T>(
    pattern: &'a syn::Pat,
    idents: &mut Vec<&'a syn::Ident>,
    errors: &mut errors::Errors<T>,
) {
    // https://doc.rust-lang.org/reference/patterns.html
    #[cfg_attr(all(test, feature = "nightly"), deny(non_exhaustive_omitted_patterns))]
    match pattern {
        syn::Pat::Lit(_) => {}
        syn::Pat::Ident(ident) => {
            // todo! on https://github.com/rust-lang/rust/issues/54725, merge the spans
            idents.push(&ident.ident);
            if let Some((_, sub)) = &ident.subpat {
                collect_idents_in_pattern(sub, idents, errors);
            }
        }
        syn::Pat::Wild(_) | syn::Pat::Rest(_) => {}
        syn::Pat::Reference(reference) => collect_idents_in_pattern(&reference.pat, idents, errors),
        syn::Pat::Struct(struct_) => {
            for field in struct_.fields.iter() {
                collect_idents_in_pattern(&field.pat, idents, errors);
            }
        }
        syn::Pat::TupleStruct(tuple_struct) => {
            for field in tuple_struct.elems.iter() {
                collect_idents_in_pattern(field, idents, errors);
            }
        }
        syn::Pat::Tuple(tuple) => {
            for field in tuple.elems.iter() {
                collect_idents_in_pattern(field, idents, errors);
            }
        }
        syn::Pat::Paren(paren) => collect_idents_in_pattern(&paren.pat, idents, errors),
        syn::Pat::Slice(slice) => {
            for field in slice.elems.iter() {
                collect_idents_in_pattern(field, idents, errors);
            }
        }
        syn::Pat::Path(_) | syn::Pat::Range(_) | syn::Pat::Macro(_) => {}
        syn::Pat::Or(or) => {
            for case in or.cases.iter() {
                collect_idents_in_pattern(case, idents, errors);
            }
        }
        syn::Pat::Const(_) => {}
        syn::Pat::Type(type_) => collect_idents_in_pattern(&type_.pat, idents, errors),
        syn::Pat::Verbatim(_) => {}
        _ => {}
    }
}

#[cfg(test)]
macro_rules! assert_syn_eq {
    ($left:expr, $right:expr) => {
        match (&$left, &$right) {
            (left, right) if left != right => {
                panic!(
                    "assertion failed: `(left == right)`\n  \
                    left: `{}`\n \
                    right: `{}`",
                    left.to_token_stream(),
                    right.to_token_stream()
                );
            }
            _ => {}
        }
    };
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::{parse_quote, Type};

    use super::*;

    #[test]
    fn sanitized_self_simple() {
        let ty = parse_quote! { Self };
        let replace_with = quote! { crate::MyCustomType<'a> };

        let sanitized: TokenStream = sanitize_self(&ty, &replace_with).collect();

        let expected = quote! { crate::MyCustomType<'a> };

        assert_eq!(sanitized.to_string(), expected.to_string());
    }

    // #[test]
    // fn sanitized_self_generic_pos() {
    //     let ty = parse_quote! { Vec<Self> };
    //     let replace_with = parse_quote! { crate::MyCustomType<'a> };

    //     let sanitized = sanitize_self(&ty, &replace_with).unwrap();

    //     let expected = parse_quote! { Vec<crate::MyCustomType<'a>> };

    //     assert_syn_eq!(sanitized, expected);
    // }

    // #[test]
    // fn sanitized_self_array() {
    //     let ty = parse_quote! { [Selfish<Self, Typed<Self>>; 4] };
    //     let replace_with = parse_quote! { crate::MyCustomType<'a> };

    //     let sanitized = sanitize_self(&ty, &replace_with).unwrap();

    //     let expected =
    //         parse_quote! { [Selfish<crate::MyCustomType<'a>, Typed<crate::MyCustomType<'a>>>; 4] };

    //     assert_syn_eq!(sanitized, expected);
    // }
}
