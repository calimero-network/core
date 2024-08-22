/// Returns `T` in `T`, `(T)` and an invisible group with T if `!deref`.
///
/// Also returns T in `&T`, `&mut T`, `(&T)`, `(&mut T)`, `(&T,)`, `(&mut T,)` and groups if `deref`
pub fn typed_path(ty: &syn::Type, deref: bool) -> Option<&syn::Path> {
    #[cfg_attr(all(test, feature = "nightly"), deny(non_exhaustive_omitted_patterns))]
    #[allow(clippy::wildcard_enum_match_arm)]
    match ty {
        syn::Type::Path(path) => Some(&path.path),
        syn::Type::Reference(reference) if deref => typed_path(&reference.elem, deref),
        syn::Type::Group(group) => typed_path(&group.elem, deref),
        syn::Type::Paren(paren) => typed_path(&paren.elem, deref),
        _ => None,
    }
}
