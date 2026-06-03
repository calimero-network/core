//! `#[derive(Migrate)]` — generate a `#[app::migrate]` function from the v2
//! state struct, so app authors write only the *diff* instead of the full
//! read-deserialize-carry skeleton.
//!
//! ```ignore
//! #[app::state]
//! #[derive(Migrate)]
//! #[migrate(from = AppV1, method = migrate_v1_to_v2)]
//! pub struct AppV2 {
//!     items: UnorderedMap<String, LwwRegister<String>>, // carried: old.items
//!     title: LwwRegister<String>,                       // carried: old.title
//!     #[migrate(new = LwwRegister::new("note".to_owned()))]
//!     notes: LwwRegister<String>,                       // additive: seeded
//!     #[migrate(from = legacy_name)]
//!     renamed: LwwRegister<String>,                     // rename: old.legacy_name
//! }
//! ```
//!
//! The generated function carries every field through from the old state by name
//! unless a `#[migrate(...)]` attribute overrides it, then runs under the same
//! `#[app::migrate]` machinery (merge mode + deterministic ids) as a hand-written
//! migration. A field with no counterpart in the old struct that is *not* marked
//! `#[migrate(new = ...)]` becomes a plain "no field `x` on the old type" compile
//! error — the developer is forced to say how new fields are seeded.
//!
//! **Dropped fields are silent** — a field present in the old struct but absent
//! from the new one is dropped with no annotation (that *is* the remove case).
//! Unlike the additive direction, nothing flags a forgotten field, so a typo'd
//! v2 definition silently discards data; review the v2 field list against the old
//! one deliberately.
//!
//! **Default method name** is `migrate`; two `#[derive(Migrate)]` in one module
//! that both omit `method = ...` collide (`error[E0428]: the name 'migrate' is
//! defined multiple times`). Give each an explicit `method = ...` (a v1->v2 and
//! v2->v3 pair always should).

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Expr, Fields, Ident, Type};

/// Per-field migration strategy, parsed from `#[migrate(...)]`.
enum FieldStrategy {
    /// No attribute — carry `old.<field>` through unchanged.
    Carry,
    /// `#[migrate(new = EXPR)]` — a field absent from the old state; seed it.
    New(Expr),
    /// `#[migrate(from = OLD_IDENT)]` — carry a renamed field: `old.<OLD_IDENT>`.
    Rename(Ident),
    /// `#[migrate(with = EXPR)]` (optionally with `from = SOURCE`) — apply a
    /// conversion `EXPR(old.<source>)`, e.g. a type change.
    With { source: Ident, expr: Expr },
}

pub fn derive(input: DeriveInput) -> TokenStream {
    match derive_inner(input) {
        Ok(ts) => ts,
        Err(e) => e.to_compile_error(),
    }
}

fn derive_inner(input: DeriveInput) -> syn::Result<TokenStream> {
    let ident = &input.ident;

    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "(calimero)> #[derive(Migrate)] supports only non-generic state structs",
        ));
    }

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new(
                    ident.span(),
                    "(calimero)> #[derive(Migrate)] requires a struct with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new(
                ident.span(),
                "(calimero)> #[derive(Migrate)] is only supported on structs",
            ))
        }
    };

    let StructArgs {
        from_type,
        method,
        emit,
    } = parse_struct_args(&input)?;

    // One `field: <expr>` initializer per v2 field. The generated locals are
    // namespaced (`__calimero_migrate_*`) so a user field of the same name can't
    // collide with the macro's bindings — derives are not identifier-hygienic.
    let mut inits: Vec<TokenStream> = Vec::with_capacity(fields.len());
    for field in fields {
        let fname = field.ident.as_ref().expect("named fields checked above");
        let init = match parse_field_strategy(field)? {
            FieldStrategy::Carry => quote! { #fname: __calimero_migrate_old.#fname },
            FieldStrategy::New(expr) => quote! { #fname: #expr },
            FieldStrategy::Rename(old) => quote! { #fname: __calimero_migrate_old.#old },
            FieldStrategy::With { source, expr } => {
                quote! { #fname: (#expr)(__calimero_migrate_old.#source) }
            }
        };
        inits.push(init);
    }

    // Optional `#[migrate(emit = EXPR)]` — emit an app event from the migration
    // (e.g. a `Migrated { from, to }` event), after the old state is read.
    let emit_stmt = match emit {
        Some(expr) => quote! { ::calimero_sdk::app::emit!(#expr); },
        None => quote! {},
    };

    Ok(quote! {
        #[::calimero_sdk::app::migrate]
        pub fn #method() -> #ident {
            let __calimero_migrate_old_bytes = ::calimero_sdk::state::read_raw().unwrap_or_else(|| {
                ::core::panic!(
                    "migrate: no existing state to migrate from (create a prior-version \
                     context first)"
                )
            });
            let __calimero_migrate_old: #from_type =
                ::calimero_sdk::borsh::BorshDeserialize::deserialize(
                    &mut &__calimero_migrate_old_bytes[..],
                )
                .unwrap_or_else(|__calimero_migrate_err| {
                    ::core::panic!(
                        "migrate: failed to deserialize prior state: {:?}",
                        __calimero_migrate_err
                    )
                });
            #emit_stmt
            #ident {
                #(#inits),*
            }
        }
    })
}

struct StructArgs {
    from_type: Type,
    method: Ident,
    emit: Option<Expr>,
}

/// Parses the struct-level `#[migrate(from = TYPE, method = IDENT, emit = EXPR)]`.
fn parse_struct_args(input: &DeriveInput) -> syn::Result<StructArgs> {
    let mut from_type: Option<Type> = None;
    let mut method: Option<Ident> = None;
    let mut emit: Option<Expr> = None;

    for attr in &input.attrs {
        if !attr.path().is_ident("migrate") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("from") {
                from_type = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("method") {
                method = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("emit") {
                emit = Some(meta.value()?.parse()?);
            } else {
                return Err(meta.error(
                    "unknown `#[migrate(...)]` option on the struct (expected `from`, `method`, \
                     or `emit`)",
                ));
            }
            Ok(())
        })?;
    }

    let from_type = from_type.ok_or_else(|| {
        syn::Error::new(
            input.ident.span(),
            "(calimero)> #[derive(Migrate)] needs `#[migrate(from = OldStateType)]` naming the \
             borsh layout of the previous version",
        )
    })?;
    let method = method.unwrap_or_else(|| Ident::new("migrate", input.ident.span()));

    Ok(StructArgs {
        from_type,
        method,
        emit,
    })
}

/// Parses a field's `#[migrate(...)]` (if any) into a [`FieldStrategy`].
fn parse_field_strategy(field: &syn::Field) -> syn::Result<FieldStrategy> {
    let mut new_expr: Option<Expr> = None;
    let mut rename: Option<Ident> = None;
    let mut with_expr: Option<Expr> = None;

    for attr in &field.attrs {
        if !attr.path().is_ident("migrate") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("new") {
                new_expr = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("from") {
                // A field rename names a single old field; reject a path with a
                // calimero-branded message rather than syn's opaque "expected `,`".
                let value = meta.value()?;
                rename = Some(value.parse().map_err(|_| {
                    meta.error(
                        "#[migrate(from = OLD_FIELD)] on a field expects a single old field \
                         name, not a path or expression",
                    )
                })?);
            } else if meta.path.is_ident("with") {
                with_expr = Some(meta.value()?.parse()?);
            } else {
                return Err(meta.error(
                    "unknown `#[migrate(...)]` option on a field (expected `new`, `from`, or \
                     `with`)",
                ));
            }
            Ok(())
        })?;
    }

    if new_expr.is_some() && (rename.is_some() || with_expr.is_some()) {
        return Err(syn::Error::new(
            field.span(),
            "(calimero)> `#[migrate(new = ...)]` (additive, no old source) can't combine with \
             `from`/`with` (which transform an existing field)",
        ));
    }

    let fname = field.ident.clone().expect("named fields checked by caller");
    match (new_expr, with_expr, rename) {
        // `with` (optionally with `from`): apply EXPR to the old source field.
        (None, Some(expr), source) => Ok(FieldStrategy::With {
            source: source.unwrap_or(fname),
            expr,
        }),
        (Some(expr), None, None) => Ok(FieldStrategy::New(expr)),
        (None, None, Some(old)) => Ok(FieldStrategy::Rename(old)),
        (None, None, None) => Ok(FieldStrategy::Carry),
        // (Some, Some, _) handled above.
        _ => unreachable!("new+with/from rejected above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn expand(ts: TokenStream) -> String {
        derive(syn::parse2(ts).expect("parse DeriveInput")).to_string()
    }

    #[test]
    fn with_applies_a_conversion_to_the_old_field() {
        let out = expand(quote! {
            #[migrate(from = AppV1)]
            struct AppV2 {
                #[migrate(with = to_string)]
                counter: String,
            }
        });
        assert!(
            out.contains("counter : (to_string) (__calimero_migrate_old . counter)"),
            "with should apply EXPR to old.counter: {out}"
        );
    }

    #[test]
    fn with_plus_from_converts_a_renamed_field() {
        let out = expand(quote! {
            #[migrate(from = AppV1)]
            struct AppV2 {
                #[migrate(from = legacy, with = convert)]
                value: String,
            }
        });
        assert!(
            out.contains("value : (convert) (__calimero_migrate_old . legacy)"),
            "with+from should apply EXPR to old.legacy: {out}"
        );
    }

    #[test]
    fn emit_generates_an_app_emit() {
        let out = expand(quote! {
            #[migrate(from = AppV1, emit = Event::Migrated { from: "1", to: "2" })]
            struct AppV2 { items: u64 }
        });
        assert!(out.contains("app :: emit"), "should emit the event: {out}");
        assert!(
            out.contains("Migrated"),
            "should emit the given event: {out}"
        );
    }

    #[test]
    fn new_and_with_on_one_field_is_error() {
        let out = expand(quote! {
            #[migrate(from = AppV1)]
            struct AppV2 {
                #[migrate(new = 0u64, with = f)]
                x: u64,
            }
        });
        assert!(out.contains("compile_error"), "{out}");
        assert!(out.contains("can't combine"), "{out}");
    }

    #[test]
    fn generates_migrate_fn_carry_new_and_rename() {
        let out = expand(quote! {
            #[migrate(from = AppV1, method = migrate_v1_to_v2)]
            struct AppV2 {
                items: u64,
                #[migrate(new = Default::default())]
                extra: u64,
                #[migrate(from = old_name)]
                renamed: u64,
            }
        });
        assert!(
            out.contains("migrate_v1_to_v2"),
            "method name missing: {out}"
        );
        assert!(out.contains("read_raw"), "should read old state: {out}");
        assert!(
            out.contains("app :: migrate"),
            "should wrap in app::migrate: {out}"
        );
        assert!(
            out.contains("__calimero_migrate_old . items"),
            "should carry items: {out}"
        );
        assert!(
            out.contains("__calimero_migrate_old . old_name"),
            "should map renamed field: {out}"
        );
        assert!(
            out.contains("Default :: default"),
            "should seed new field: {out}"
        );
    }

    #[test]
    fn defaults_method_name_to_migrate() {
        let out = expand(quote! {
            #[migrate(from = AppV1)]
            struct AppV2 { items: u64 }
        });
        assert!(
            out.contains("fn migrate ("),
            "default method name `migrate`: {out}"
        );
    }

    #[test]
    fn missing_from_is_compile_error() {
        let out = expand(quote! {
            struct AppV2 { items: u64 }
        });
        assert!(out.contains("compile_error"), "{out}");
        assert!(out.contains("migrate(from"), "names the fix: {out}");
    }

    #[test]
    fn new_and_from_on_one_field_is_error() {
        let out = expand(quote! {
            #[migrate(from = AppV1)]
            struct AppV2 {
                #[migrate(new = 0u64, from = old)]
                x: u64,
            }
        });
        assert!(out.contains("compile_error"), "{out}");
        assert!(out.contains("can't combine"), "{out}");
    }

    #[test]
    fn generic_struct_is_rejected() {
        let out = expand(quote! {
            #[migrate(from = AppV1)]
            struct AppV2<T> { items: T }
        });
        assert!(out.contains("compile_error"), "{out}");
        assert!(out.contains("non-generic"), "{out}");
    }

    #[test]
    fn user_field_named_old_does_not_collide_with_generated_local() {
        // A field literally named `__old` (what a naive macro would bind) must
        // carry cleanly; the macro's locals are `__calimero_migrate_*`.
        let out = expand(quote! {
            #[migrate(from = AppV1)]
            struct AppV2 { __old: u64 }
        });
        assert!(
            out.contains("__old : __calimero_migrate_old . __old"),
            "field __old should carry from old.__old via the namespaced local: {out}"
        );
    }
}
