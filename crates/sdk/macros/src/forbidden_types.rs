//! Shared compile-time check that rejects non-mergeable types in persistent state.
//!
//! Used by `#[app::state]` and `#[derive(Mergeable)]` to make sure every field
//! either is a Calimero CRDT collection (or a `LwwRegister<T>` / `Option<T>` /
//! user-derived `Mergeable` struct) — never a `std::collections::HashMap`, a
//! bare `Vec`, a bare `String`, or a primitive. Without this check, those fields
//! silently fail to merge and replicas drift apart.

use syn::spanned::Spanned;
use syn::{Fields, GenericArgument, PathArguments, Type};

use crate::errors::{Errors, ParseError};

/// Recursively validates the type of a single field. The `is_top_level` flag is
/// true for the field's own type, false for anything reached via a generic
/// argument or tuple element — the rules differ at the root vs. nested.
pub fn validate_field_type<T>(ty: &Type, errors: &Errors<'_, T>, is_top_level: bool) {
    match ty {
        Type::Path(tp) => {
            let Some(last) = tp.path.segments.last() else {
                return;
            };
            let ident_str = last.ident.to_string();

            if let Some(suggestion) = forbidden_std_collection(&ident_str) {
                errors.subsume(syn::Error::new(
                    last.ident.span(),
                    ParseError::ForbiddenStdCollection {
                        type_name: leak_str(ident_str.clone()),
                        suggestion,
                    },
                ));
            } else if is_top_level {
                if let Some(suggestion) = forbidden_top_level_bare(&ident_str) {
                    errors.subsume(syn::Error::new(
                        last.ident.span(),
                        ParseError::ForbiddenBarePrimitive {
                            type_name: leak_str(ident_str.clone()),
                            suggestion,
                        },
                    ));
                }
            }

            // Wrappers that pass through to their inner type for top-level checks.
            // `Option<T>` and `Box<T>` don't change merge semantics; the inner
            // type is what carries them.
            let pass_through = matches!(ident_str.as_str(), "Option" | "Box");

            if let PathArguments::AngleBracketed(args) = &last.arguments {
                for arg in &args.args {
                    if let GenericArgument::Type(inner) = arg {
                        validate_field_type(inner, errors, is_top_level && pass_through);
                    }
                }
            }
        }
        Type::Tuple(t) => {
            for elem in &t.elems {
                validate_field_type(elem, errors, false);
            }
        }
        Type::Array(t) => validate_field_type(&t.elem, errors, false),
        Type::Group(g) => validate_field_type(&g.elem, errors, is_top_level),
        Type::Paren(p) => validate_field_type(&p.elem, errors, is_top_level),
        Type::Reference(r) => validate_field_type(&r.elem, errors, false),
        _ => {}
    }
}

/// Validate every field in a `Fields` block (struct or enum variant).
pub fn validate_fields<T>(fields: &Fields, errors: &Errors<'_, T>) {
    for field in fields.iter() {
        validate_field_type(&field.ty, errors, true);
    }
}

/// Std-collection types whose presence anywhere in a field's type is wrong:
/// they would be persisted as opaque blobs with no merge semantics, even when
/// nested inside an SDK wrapper like `LwwRegister<HashMap<...>>`.
fn forbidden_std_collection(ident: &str) -> Option<&'static str> {
    match ident {
        "HashMap" => Some("UnorderedMap<K, V>"),
        "BTreeMap" => Some("UnorderedMap<K, V>"),
        "HashSet" => Some("UnorderedSet<T>"),
        "BTreeSet" => Some("UnorderedSet<T>"),
        "LinkedList" => Some("Vector<T>"),
        "VecDeque" => Some("Vector<T>"),
        _ => None,
    }
}

/// Types that are illegal as the *root* of a field but may appear nested inside
/// SDK wrappers (e.g. `UnorderedMap<String, _>` keeps `String` at depth 1, which
/// is fine because the trait bound `V: Mergeable` constrains the value side).
fn forbidden_top_level_bare(ident: &str) -> Option<&'static str> {
    match ident {
        "Vec" => Some("Vector<T>"),
        "String" => Some("LwwRegister<String>"),
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128"
        | "isize" => Some("LwwRegister<T> for last-write-wins, or Counter for monotonic counters"),
        "f32" | "f64" => Some("LwwRegister<T>"),
        "bool" | "char" => Some("LwwRegister<T>"),
        _ => None,
    }
}

/// `ParseError` variants take `&'static str` (a thiserror constraint we share
/// with the rest of the codebase). Field idents are owned `String`s, so we leak
/// them — proc-macros are short-lived processes, the leak is bounded by the
/// number of forbidden fields the user wrote. That's fine.
fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Helper used by `Span::call_site()` callers when no concrete span is available.
#[allow(dead_code)]
pub fn span_for(ty: &Type) -> proc_macro2::Span {
    ty.span()
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;
    use crate::errors::Errors;

    fn check(ty: Type) -> Option<String> {
        let errors: Errors<'_> = Errors::default();
        validate_field_type(&ty, &errors, true);
        errors.take().map(|e| e.to_string())
    }

    #[test]
    fn allowed_unordered_map() {
        assert!(check(parse_quote!(UnorderedMap<String, LwwRegister<String>>)).is_none());
    }

    #[test]
    fn allowed_vector_of_lww() {
        assert!(check(parse_quote!(Vector<LwwRegister<u64>>)).is_none());
    }

    #[test]
    fn allowed_counter() {
        assert!(check(parse_quote!(Counter)).is_none());
    }

    #[test]
    fn allowed_option_of_crdt() {
        assert!(check(parse_quote!(
            Option<UnorderedMap<String, LwwRegister<String>>>
        ))
        .is_none());
    }

    #[test]
    fn rejects_top_level_string() {
        let err = check(parse_quote!(String)).expect("should error");
        assert!(err.contains("`String`"), "{err}");
        assert!(err.contains("LwwRegister<String>"), "{err}");
    }

    #[test]
    fn rejects_top_level_u64() {
        let err = check(parse_quote!(u64)).expect("should error");
        assert!(err.contains("`u64`"), "{err}");
        assert!(err.contains("Counter"), "{err}");
    }

    #[test]
    fn rejects_top_level_vec() {
        let err = check(parse_quote!(Vec<u8>)).expect("should error");
        assert!(err.contains("`Vec`"), "{err}");
        assert!(err.contains("Vector<T>"), "{err}");
    }

    #[test]
    fn rejects_hashmap_at_top_level() {
        let err = check(parse_quote!(HashMap<String, String>)).expect("should error");
        assert!(err.contains("`HashMap`"), "{err}");
        assert!(err.contains("UnorderedMap"), "{err}");
    }

    #[test]
    fn rejects_hashmap_via_full_path() {
        let err =
            check(parse_quote!(std::collections::HashMap<String, String>)).expect("should error");
        assert!(err.contains("`HashMap`"), "{err}");
    }

    #[test]
    fn rejects_btree_set_anywhere() {
        let err = check(parse_quote!(LwwRegister<BTreeSet<u64>>)).expect("should error");
        assert!(err.contains("`BTreeSet`"), "{err}");
    }

    #[test]
    fn rejects_hashmap_inside_lww_register() {
        // The blob-via-LwwRegister escape hatch is exactly the bug we're trying
        // to catch: `LwwRegister<T>` makes the inner type Mergeable as a whole,
        // so a HashMap inside would silently compile but never CRDT-merge.
        let err = check(parse_quote!(LwwRegister<HashMap<String, u64>>)).expect("should error");
        assert!(err.contains("`HashMap`"), "{err}");
    }

    #[test]
    fn rejects_hashmap_inside_unordered_map_value() {
        let err =
            check(parse_quote!(UnorderedMap<String, HashMap<String, u64>>)).expect("should error");
        assert!(err.contains("`HashMap`"), "{err}");
    }

    #[test]
    fn rejects_vec_deque_and_linked_list() {
        assert!(check(parse_quote!(VecDeque<u8>)).is_some());
        assert!(check(parse_quote!(LinkedList<u8>)).is_some());
    }

    #[test]
    fn allows_string_as_unordered_map_key() {
        // Keys aren't mergeable — they're stored as bytes — so `String` at depth
        // 1 inside `UnorderedMap` is fine. Only top-level bare `String` is wrong.
        assert!(check(parse_quote!(UnorderedMap<String, LwwRegister<String>>)).is_none());
    }

    #[test]
    fn rejects_bare_bool() {
        assert!(check(parse_quote!(bool)).is_some());
    }

    #[test]
    fn allows_user_struct_at_top_level() {
        // Unknown idents are presumed to be user types; the trait bound on the
        // collection ensures they implement Mergeable at the use site.
        assert!(check(parse_quote!(MyCustomStruct)).is_none());
        assert!(check(parse_quote!(LwwRegister<MyCustomStruct>)).is_none());
    }
}
