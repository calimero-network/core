//! The shapes of an app's own types in its ABI (`types`): the fields of a record, the
//! variants of an enum, as their JSON travels.
//!
//! `#[derive(Record)]` describes a record, `#[derive(AbiType)]` a serde struct or enum (it
//! reads the `#[serde(..)]` attributes that change the JSON). `#[app::logic]` lists every
//! type its methods take or answer, and the app's errors and events carry, with the types
//! those refer to, under `types` in the ABI, sorted by name. A type without either derive is
//! left out, and clients type it `unknown`, as before.
//!
//! Each shape is canonical JSON (keys sorted), assembled at compile time:
//!
//! - `{"fields":[[name,type],..],"kind":"record"}`; a field the JSON may leave out is
//!   `[name,type,{"optional":true}]`;
//! - `{"kind":"newtype","type":type}`, `{"items":[type,..],"kind":"tuple"}`, `{"kind":"unit"}`;
//! - `{"kind":"enum","variants":[..]}`, each variant `{"name":n}` (unit), `{"name":n,"type":t}`
//!   (newtype), `{"items":[..],"name":n}` (tuple) or `{"fields":[..],"name":n}` (struct), with
//!   serde's representation: externally tagged by default, `"tag"` (internally), `"tag"` and
//!   `"content"` (adjacently), or `"untagged":true`;
//! - a generic type adds `"params":["T",..]`, and its fields name them (`list<T>`).
//!
//! Types are ABI type strings, with `u64`/`i64` decimal strings and `bytes` base64url (D20).

/// A type's entry in the ABI's `types`, and the types its shape refers to.
#[derive(Debug)]
pub struct TypeDef {
    /// The key in `types`: the Rust type's name, without generic arguments.
    pub name: &'static str,
    /// Its shape, canonical JSON.
    pub shape: &'static str,
    /// What the shape refers to; `None` for a type without a shape.
    pub deps: &'static [Option<&'static TypeDef>],
}

/// A type whose shape the ABI carries: written by `#[derive(Record)]` and
/// `#[derive(AbiType)]`. Implement it by hand only for a type with a hand-written serde form
/// (the SDK does for `BlobRef`).
pub trait AbiType {
    /// The type's entry.
    const DEF: &'static TypeDef;
}

/// The most types one ABI lists.
pub const MAX_TYPES: usize = 256;
/// The most bytes their shapes take, together.
pub const MAX_TYPES_BYTES: usize = 128 * 1024;

/// The `types` section as the ABI writes it: `,"types":{..}` sorted by name, or nothing when
/// no type has a shape (so an app without one keeps the ABI it had).
#[derive(Debug)]
pub struct TypesJson {
    buf: [u8; MAX_TYPES_BYTES],
    len: usize,
}

impl TypesJson {
    /// The text, for `abi_concat`.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        let (text, _) = self.buf.split_at(self.len);
        match core::str::from_utf8(text) {
            Ok(text) => text,
            // SAFETY: guaranteed by `types_json`, which copies whole `&str`s only.
            Err(_) => panic!("types: not UTF-8"),
        }
    }
}

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn before(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut i = 0;
    while i < a.len() && i < b.len() {
        if a[i] != b[i] {
            return a[i] < b[i];
        }
        i += 1;
    }
    a.len() < b.len()
}

const fn push(buf: &mut [u8; MAX_TYPES_BYTES], len: &mut usize, text: &str) {
    let bytes = text.as_bytes();
    assert!(
        *len + bytes.len() <= MAX_TYPES_BYTES,
        "the app's type shapes exceed MAX_TYPES_BYTES"
    );
    let mut i = 0;
    while i < bytes.len() {
        buf[*len] = bytes[i];
        *len += 1;
        i += 1;
    }
}

/// Every type `roots` refer to, directly or through other shapes, written as the ABI's
/// `types`. Two types of one name keep the first met.
#[must_use]
pub const fn types_json(roots: &[&[Option<&'static TypeDef>]]) -> TypesJson {
    let mut found: [Option<&'static TypeDef>; MAX_TYPES] = [None; MAX_TYPES];
    let mut count = 0;
    let mut stack: [Option<&'static TypeDef>; MAX_TYPES * 16] = [None; MAX_TYPES * 16];
    let mut top = 0;
    let mut r = 0;
    while r < roots.len() {
        let mut i = 0;
        while i < roots[r].len() {
            assert!(top < stack.len(), "the app's types exceed MAX_TYPES");
            stack[top] = roots[r][i];
            top += 1;
            i += 1;
        }
        r += 1;
    }
    while top > 0 {
        top -= 1;
        let Some(def) = stack[top] else { continue };
        let mut seen = false;
        let mut k = 0;
        while k < count {
            if let Some(other) = found[k] {
                if same(other.name, def.name) {
                    seen = true;
                }
            }
            k += 1;
        }
        if seen {
            continue;
        }
        assert!(count < MAX_TYPES, "the app's types exceed MAX_TYPES");
        found[count] = Some(def);
        count += 1;
        let mut d = 0;
        while d < def.deps.len() {
            assert!(top < stack.len(), "the app's types exceed MAX_TYPES");
            stack[top] = def.deps[d];
            top += 1;
            d += 1;
        }
    }
    // Sorted by name: insertion sort, as `const fn` allows.
    let mut i = 1;
    while i < count {
        let mut j = i;
        while j > 0 {
            let (Some(a), Some(b)) = (found[j - 1], found[j]) else {
                break;
            };
            if !before(b.name, a.name) {
                break;
            }
            found[j - 1] = Some(b);
            found[j] = Some(a);
            j -= 1;
        }
        i += 1;
    }
    let mut buf = [0u8; MAX_TYPES_BYTES];
    let mut len = 0;
    if count > 0 {
        push(&mut buf, &mut len, ",\"types\":{");
        let mut k = 0;
        while k < count {
            if let Some(def) = found[k] {
                if k > 0 {
                    push(&mut buf, &mut len, ",");
                }
                push(&mut buf, &mut len, "\"");
                push(&mut buf, &mut len, def.name);
                push(&mut buf, &mut len, "\":");
                push(&mut buf, &mut len, def.shape);
            }
            k += 1;
        }
        push(&mut buf, &mut len, "}");
    }
    TypesJson { buf, len }
}

/// `BlobRef` travels as its id, a string.
impl AbiType for crate::BlobRef {
    const DEF: &'static TypeDef = &TypeDef {
        name: "BlobRef",
        shape: r#"{"kind":"newtype","type":"string"}"#,
        deps: &[],
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A;
    struct B;
    struct Plain;

    impl AbiType for A {
        const DEF: &'static TypeDef = &TypeDef {
            name: "A",
            shape: r#"{"fields":[["b","B"]],"kind":"record"}"#,
            deps: &[crate::__private::Probe::<B>::DEF, {
                use crate::__private::NoShape as _;
                crate::__private::Probe::<Plain>::DEF
            }],
        };
    }

    impl AbiType for B {
        const DEF: &'static TypeDef = &TypeDef {
            name: "B",
            shape: r#"{"kind":"unit"}"#,
            deps: &[],
        };
    }

    #[test]
    fn types_are_found_through_shapes_sorted_and_listed_once() {
        const ROOTS: &[Option<&TypeDef>] = &[
            crate::__private::Probe::<A>::DEF,
            crate::__private::Probe::<B>::DEF,
        ];
        const JSON: TypesJson = types_json(&[ROOTS, &[]]);
        assert_eq!(
            JSON.as_str(),
            r#","types":{"A":{"fields":[["b","B"]],"kind":"record"},"B":{"kind":"unit"}}"#
        );
    }

    #[test]
    fn no_shape_writes_nothing() {
        use crate::__private::NoShape as _;
        const ROOTS: &[Option<&TypeDef>] = &[crate::__private::Probe::<Plain>::DEF];
        const JSON: TypesJson = types_json(&[ROOTS]);
        assert_eq!(JSON.as_str(), "");
    }
}
