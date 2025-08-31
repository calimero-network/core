use syn::{Path, Type};

/// Returns `T` in `T`, `(T)` and an invisible group with T if `!deref`.
///
/// Also returns T in `&T`, `&mut T`, `(&T)`, `(&mut T)`, `(&T,)`, `(&mut T,)` and groups if `deref`
pub fn typed_path(ty: &Type, deref: bool) -> Option<&Path> {
    match ty {
        Type::Path(path) => Some(&path.path),
        Type::Reference(reference) if deref => typed_path(&reference.elem, deref),
        Type::Group(group) => typed_path(&group.elem, deref),
        Type::Paren(paren) => typed_path(&paren.elem, deref),
        Type::Array(_) => None,
        Type::BareFn(_) => None,
        Type::ImplTrait(_) => None,
        Type::Infer(_) => None,
        Type::Macro(_) => None,
        Type::Never(_) => None,
        Type::Ptr(_) => None,
        Type::Slice(_) => None,
        Type::TraitObject(_) => None,
        Type::Tuple(_) => None,
        Type::Verbatim(_) => None,
        _ => None,
    }
}
