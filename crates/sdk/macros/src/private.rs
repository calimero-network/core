use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::{
    parse_quote, Error as SynError, Fields, GenericArgument, Ident, PathArguments,
    Result as SynResult, Token, Type,
};

use crate::errors::{Errors, ParseError};
use crate::items::StructOrEnumItem;
use crate::reserved::idents;
use sha2::{Digest, Sha256};

/// Tree-backed *structural* collections that store entities in the
/// shared Merkle tree by default, paired with the number of type
/// arguments their no-adaptor form takes.
///
/// When a field of one of these types appears (at any depth) inside
/// a `#[app::private]` struct, the macro injects `PrivateStorage` as
/// the trailing generic so the entries land in the node-local
/// namespace instead of leaking into the synced tree.
///
/// **Single source of truth.** Name and arity live together so we
/// never end up "in the name list but missing from the arity match"
/// — that drift was a real maintenance hazard with two separate
/// tables.
///
/// **Scope**: only structural collections (`UnorderedMap`,
/// `UnorderedSet`, `Vector`) — primitives and `std` types
/// (`String`, `u64`, `BTreeMap`, `Vec`, etc.) borsh-serialise
/// straight into the outer private blob and need no substitution.
///
/// **Deliberately excluded** (using them inside `#[app::private]`
/// is a semantic mismatch and currently produces a normal type
/// error, since their `::new()` stays pinned to `MainStorage`; a
/// dedicated compile-time diagnostic is tracked in #2428):
///
/// - CRDT data-types (`LwwRegister`, `Counter`, `GCounter`,
///   `PNCounter`, `ReplicatedGrowableArray`) — CRDTs exist for
///   multi-writer conflict resolution; in single-writer private
///   storage their merge semantics are unused complexity.
/// - Access-control collections (`SharedStorage`, `UserStorage`,
///   `FrozenStorage`) — cross-writer mutability, per-user
///   separation, and immutability all assume the synced tree.
/// - Authored collections (`AuthoredMap`, `AuthoredVector`) —
///   carry per-entry authorship which is a sync-side concern.
///
/// Matched on the *last segment* of the type path, so callers using
/// fully-qualified paths (`calimero_storage::collections::UnorderedMap`)
/// are also covered. Doesn't catch type aliases or `use as` renames —
/// users doing those need to either write the explicit `PrivateStorage`
/// parameter themselves or unwind the alias.
const TREE_BACKED_TYPES: &[(&str, usize)] = &[
    ("UnorderedMap", 2),
    ("SortedMap", 2),
    ("UnorderedSet", 1),
    ("Vector", 1),
];

/// Recursively walk `ty` and inject `PrivateStorage` on every
/// tree-backed collection encountered — including ones nested inside
/// other generics (`Option<UnorderedMap<K, V>>`,
/// `UnorderedMap<K, Vector<T>>`, `(UnorderedMap<K, V>, Vector<T>)`,
/// `Box<UnorderedSet<T>>`). Without this recursion the outer field
/// type is privatised but the inner collection silently keeps its
/// default `MainStorage`, and its entries leak into the synced tree.
///
/// We recurse before deciding whether to inject on the outer node
/// so the inner rewrites are visible in the resulting token stream.
fn inject_private_storage(ty: &mut Type) {
    match ty {
        Type::Path(type_path) => {
            if let Some(last) = type_path.path.segments.last_mut() {
                if let PathArguments::AngleBracketed(args) = &mut last.arguments {
                    for arg in args.args.iter_mut() {
                        if let GenericArgument::Type(inner) = arg {
                            inject_private_storage(inner);
                        }
                    }
                }
            }
            try_inject_on_path(type_path);
        }
        Type::Reference(r) => inject_private_storage(&mut r.elem),
        Type::Tuple(t) => {
            for elem in t.elems.iter_mut() {
                inject_private_storage(elem);
            }
        }
        Type::Array(a) => inject_private_storage(&mut a.elem),
        Type::Slice(s) => inject_private_storage(&mut s.elem),
        Type::Group(g) => inject_private_storage(&mut g.elem),
        Type::Paren(p) => inject_private_storage(&mut p.elem),
        // BareFn / Ptr / TraitObject / ImplTrait / Infer / Macro / Never
        // / Verbatim — nothing structural to walk into for our purpose.
        _ => {}
    }
}

/// Inject `PrivateStorage` on a path node if it names a known
/// tree-backed type with the right arity. Detection is "did the
/// user supply enough generics for an explicit adaptor" — for
/// `UnorderedMap<K, V>` we inject; for `UnorderedMap<K, V, SomeStorage>`
/// we don't touch it.
///
/// The supported collections take only `Type` generics in their
/// public API (no lifetimes, no const generics *on the collection
/// itself*). If any lifetime/const argument appears at the *top
/// level* of the path's generics — e.g. someone wrote
/// `UnorderedMap<'a, K, V>` — we conservatively bail rather than
/// risk producing a malformed type. Lifetimes/consts nested inside
/// the type arguments themselves (e.g. `UnorderedMap<String, &'a str>`)
/// don't affect arity and are not what this bail catches.
fn try_inject_on_path(type_path: &mut syn::TypePath) {
    let Some(last) = type_path.path.segments.last_mut() else {
        return;
    };
    let name = last.ident.to_string();
    let Some(&(_, expected)) = TREE_BACKED_TYPES.iter().find(|(n, _)| *n == name) else {
        return;
    };

    let private_storage: GenericArgument = parse_quote!(::calimero_storage::store::PrivateStorage);

    match &mut last.arguments {
        // None of the supported types are valid with zero type-args
        // (UnorderedMap needs K,V; UnorderedSet/Vector need T) — a
        // bare identifier from the list is malformed Rust anyway, so
        // we leave it alone and let the type-checker produce the more
        // informative error.
        PathArguments::None => {}
        PathArguments::AngleBracketed(args) => {
            // The recursion in `inject_private_storage` runs BEFORE
            // this match, so by the time we count args, any nested
            // tree-backed collection has already had its own
            // `PrivateStorage` injected. That doesn't affect this
            // count: each nested `Type` (rewritten or not) is still a
            // single `GenericArgument::Type` at the OUTER level — its
            // internal arity doesn't leak. So
            // `UnorderedMap<K, UnorderedMap<X, Y>>` always counts as
            // 2 type args here, whether the inner map was rewritten
            // to 3 args or not.
            let mut type_args = 0_usize;
            let mut other_args = 0_usize;
            for arg in &args.args {
                match arg {
                    GenericArgument::Type(_) => type_args += 1,
                    _ => other_args += 1,
                }
            }
            if other_args == 0 && type_args == expected {
                args.args.push(private_storage);
            }
        }
        PathArguments::Parenthesized(_) => {}
    }
}

/// Walk every field of `orig` and rewrite tree-backed collection
/// types in place. Returns the rewritten clone for emission. Enums
/// are walked too — variant fields are treated identically to struct
/// fields.
fn rewrite_tree_backed_field_types(orig: &StructOrEnumItem) -> StructOrEnumItem {
    let mut rewritten = orig.clone();
    match &mut rewritten {
        StructOrEnumItem::Struct(item) => match &mut item.fields {
            Fields::Named(fields) => {
                for field in &mut fields.named {
                    inject_private_storage(&mut field.ty);
                }
            }
            Fields::Unnamed(fields) => {
                for field in &mut fields.unnamed {
                    inject_private_storage(&mut field.ty);
                }
            }
            Fields::Unit => {}
        },
        StructOrEnumItem::Enum(item) => {
            for variant in &mut item.variants {
                match &mut variant.fields {
                    Fields::Named(fields) => {
                        for field in &mut fields.named {
                            inject_private_storage(&mut field.ty);
                        }
                    }
                    Fields::Unnamed(fields) => {
                        for field in &mut fields.unnamed {
                            inject_private_storage(&mut field.ty);
                        }
                    }
                    Fields::Unit => {}
                }
            }
        }
    }
    rewritten
}

pub struct PrivateImpl<'a> {
    ident: &'a Ident,
    key_name: String,
    key_bytes: Vec<u8>,
    /// `orig` with every tree-backed collection field rewritten to
    /// use `PrivateStorage` as its storage adaptor — see
    /// [`rewrite_tree_backed_field_types`].
    rewritten: StructOrEnumItem,
}

impl ToTokens for PrivateImpl<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let PrivateImpl {
            ident,
            key_name,
            key_bytes,
            rewritten,
        } = self;

        let key_bytes_literal = proc_macro2::Literal::byte_string(key_bytes);
        let key_name_ident = Ident::new(key_name, ident.span());

        quote! {
            #rewritten

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

        // Generate a default key if none provided (hash ident to avoid collisions)
        let key_bytes = input
            .args
            .key
            .clone()
            .unwrap_or_else(|| compute_default_key(ident));

        // Generate key name
        let key_name = format!("{}_KEY", ident.to_string().to_uppercase());

        errors.check()?;

        let rewritten = rewrite_tree_backed_field_types(input.item);

        Ok(PrivateImpl {
            ident,
            key_name,
            key_bytes,
            rewritten,
        })
    }
}

fn compute_default_key(ident: &Ident) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(ident.to_string().as_bytes());
    let digest = hasher.finalize();
    digest[..32].to_vec()
}

#[cfg(test)]
mod tests {
    use super::{compute_default_key, inject_private_storage};
    use quote::ToTokens;
    use syn::{parse_quote, parse_str, Type};

    #[test]
    fn default_key_uses_hash_no_prefix_collision() {
        let a: syn::Ident =
            parse_str("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAX").unwrap();
        let b: syn::Ident =
            parse_str("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAY").unwrap();
        // Both share a long common prefix; hashing should still yield different keys
        let ka = compute_default_key(&a);
        let kb = compute_default_key(&b);
        assert_ne!(ka, kb);
        assert_eq!(ka.len(), 32);
        assert_eq!(kb.len(), 32);
    }

    fn rewrite(input: Type) -> String {
        let mut ty = input;
        inject_private_storage(&mut ty);
        ty.to_token_stream().to_string()
    }

    #[test]
    fn map_with_two_args_gets_private_storage_appended() {
        // `UnorderedMap<K, V>` → `UnorderedMap<K, V, ::calimero_storage::store::PrivateStorage>`
        let rewritten = rewrite(parse_quote!(UnorderedMap<String, String>));
        assert!(
            rewritten.contains("PrivateStorage"),
            "private storage should be appended, got: {rewritten}"
        );
    }

    #[test]
    fn map_with_explicit_adaptor_is_left_alone() {
        // User already supplied an adaptor — don't touch.
        let rewritten = rewrite(parse_quote!(UnorderedMap<String, String, SomeOtherAdaptor>));
        assert!(
            !rewritten.contains("PrivateStorage"),
            "explicit adaptor should not be overridden, got: {rewritten}"
        );
        assert!(
            rewritten.contains("SomeOtherAdaptor"),
            "original adaptor should be preserved, got: {rewritten}"
        );
    }

    #[test]
    fn unknown_type_is_left_alone() {
        // `BTreeMap<K, V>` is not a tree-backed Calimero collection — leave it.
        let rewritten = rewrite(parse_quote!(BTreeMap<String, String>));
        assert!(
            !rewritten.contains("PrivateStorage"),
            "non-Calimero types must not be rewritten, got: {rewritten}"
        );
    }

    #[test]
    fn vector_with_one_arg_gets_private_storage() {
        let rewritten = rewrite(parse_quote!(Vector<String>));
        assert!(
            rewritten.contains("PrivateStorage"),
            "Vector should be rewritten, got: {rewritten}"
        );
    }

    #[test]
    fn unordered_set_with_one_arg_gets_private_storage() {
        let rewritten = rewrite(parse_quote!(UnorderedSet<String>));
        assert!(
            rewritten.contains("PrivateStorage"),
            "UnorderedSet should be rewritten, got: {rewritten}"
        );
    }

    // Single source of truth for "what we deliberately don't
    // rewrite." Lifted to module level so the exclusion list is
    // grep-able from one place rather than buried inside each test
    // body. The doc-comment above `TREE_BACKED_TYPES` lists the same
    // names; if these ever drift, the inconsistency-with-prose
    // becomes the user-facing bug.
    //
    // If a future contributor adds (say) `AuthoredMap` to
    // `TREE_BACKED_TYPES`, the corresponding entry here flips its
    // assertion red and the change has to be made consciously.

    const EXCLUDED_CRDT_TYPES: &[&str] = &[
        "LwwRegister<String>",
        "Counter",
        "GCounter",
        "PNCounter",
        "ReplicatedGrowableArray<String>",
    ];

    const EXCLUDED_ACCESS_CONTROL_TYPES: &[&str] = &[
        "SharedStorage<String>",
        "UserStorage<String>",
        "FrozenStorage<String>",
        "AuthoredMap<String, String>",
        "AuthoredVector<String>",
    ];

    #[test]
    fn crdt_type_is_left_alone() {
        // CRDT collections deliberately excluded from substitution —
        // they're multi-writer machinery that has no place in
        // single-writer private storage.
        for ty in EXCLUDED_CRDT_TYPES {
            let parsed: Type = syn::parse_str(ty).expect("parse");
            let rewritten = rewrite(parsed);
            assert!(
                !rewritten.contains("PrivateStorage"),
                "CRDT type {ty} must not be rewritten, got: {rewritten}"
            );
        }
    }

    #[test]
    fn access_control_types_are_left_alone() {
        // Shared/User/Frozen/Authored — semantics rely on the synced
        // tree, deliberately excluded.
        for ty in EXCLUDED_ACCESS_CONTROL_TYPES {
            let parsed: Type = syn::parse_str(ty).expect("parse");
            let rewritten = rewrite(parsed);
            assert!(
                !rewritten.contains("PrivateStorage"),
                "Access-control type {ty} must not be rewritten, got: {rewritten}"
            );
        }
    }

    #[test]
    fn excluded_type_names_disjoint_from_tree_backed() {
        // Belt-and-braces machine check that the exclusion lists
        // above don't accidentally name something the macro would
        // rewrite — the strongest signal that the prose doc and the
        // actual constant agree.
        let included: std::collections::HashSet<&str> =
            super::TREE_BACKED_TYPES.iter().map(|(n, _)| *n).collect();
        for excluded in EXCLUDED_CRDT_TYPES
            .iter()
            .chain(EXCLUDED_ACCESS_CONTROL_TYPES)
        {
            // Pull just the leading identifier (everything before `<`).
            let head = excluded.split('<').next().expect("non-empty");
            assert!(
                !included.contains(head),
                "{head} is in both TREE_BACKED_TYPES and an EXCLUDED_* list — pick one"
            );
        }
    }

    // Nested-rewrite tests: the macro must descend into generic
    // arguments, tuples, references, etc., or inner collections silently
    // leak into the synced tree.

    fn count_occurrences(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    #[test]
    fn nested_map_in_map_value_is_rewritten() {
        // Both the outer and inner map must end up with PrivateStorage.
        let rewritten = rewrite(parse_quote!(UnorderedMap<String, UnorderedMap<String, String>>));
        assert_eq!(
            count_occurrences(&rewritten, "PrivateStorage"),
            2,
            "both outer and inner maps must be rewritten, got: {rewritten}"
        );
    }

    #[test]
    fn nested_vector_in_map_value_is_rewritten() {
        let rewritten = rewrite(parse_quote!(UnorderedMap<String, Vector<u8>>));
        assert_eq!(
            count_occurrences(&rewritten, "PrivateStorage"),
            2,
            "both map and nested vector must be rewritten, got: {rewritten}"
        );
    }

    #[test]
    fn option_wrapped_collection_is_rewritten() {
        // `Option<UnorderedMap<K, V>>` — the wrapper is borsh-serialised
        // into the blob, but the inner collection stores via the
        // adaptor, so it must be rewritten or the entries leak.
        let rewritten = rewrite(parse_quote!(Option<UnorderedMap<String, String>>));
        assert_eq!(
            count_occurrences(&rewritten, "PrivateStorage"),
            1,
            "Option-wrapped inner collection must be rewritten, got: {rewritten}"
        );
    }

    #[test]
    fn vec_wrapped_collection_is_rewritten() {
        let rewritten = rewrite(parse_quote!(Vec<Vector<u64>>));
        assert_eq!(
            count_occurrences(&rewritten, "PrivateStorage"),
            1,
            "Vec-wrapped inner collection must be rewritten, got: {rewritten}"
        );
    }

    #[test]
    fn box_wrapped_collection_is_rewritten() {
        let rewritten = rewrite(parse_quote!(Box<UnorderedSet<String>>));
        assert_eq!(
            count_occurrences(&rewritten, "PrivateStorage"),
            1,
            "Box-wrapped inner collection must be rewritten, got: {rewritten}"
        );
    }

    #[test]
    fn tuple_elements_are_each_rewritten() {
        let rewritten = rewrite(parse_quote!((
            UnorderedMap<String, String>,
            Vector<u64>,
            BTreeMap<u8, u8>
        )));
        assert_eq!(
            count_occurrences(&rewritten, "PrivateStorage"),
            2,
            "tuple elements must each be rewritten, got: {rewritten}"
        );
    }

    #[test]
    fn reference_inner_is_rewritten() {
        // Doesn't make sense as a struct field in practice, but a
        // useful pin: the walker descends through Type::Reference.
        let rewritten = rewrite(parse_quote!(&UnorderedMap<String, String>));
        assert_eq!(
            count_occurrences(&rewritten, "PrivateStorage"),
            1,
            "reference inner must be rewritten, got: {rewritten}"
        );
    }

    #[test]
    fn deeply_nested_collections_all_get_rewritten() {
        let rewritten = rewrite(parse_quote!(UnorderedMap<String, Option<Vec<Vector<u64>>>>));
        assert_eq!(
            count_occurrences(&rewritten, "PrivateStorage"),
            2,
            "every nested tree-backed collection must be rewritten, got: {rewritten}"
        );
    }
}
