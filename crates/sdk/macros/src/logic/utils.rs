use syn::{Path, Type};

/// Returns `T` in `T`, `(T)` and an invisible group with T if `!deref`.
///
/// Also returns T in `&T`, `&mut T`, `(&T)`, `(&mut T)`, `(&T,)`, `(&mut T,)` and groups if `deref`
pub fn typed_path(ty: &Type, deref: bool) -> Option<&Path> {
    #[cfg_attr(all(test, feature = "nightly"), deny(non_exhaustive_omitted_patterns))]
    #[expect(clippy::wildcard_enum_match_arm, reason = "This is reasonable here")]
    match ty {
        Type::Path(path) => Some(&path.path),
        Type::Reference(reference) if deref => typed_path(&reference.elem, deref),
        Type::Group(group) => typed_path(&group.elem, deref),
        Type::Paren(paren) => typed_path(&paren.elem, deref),
        _ => None,
    }
}
