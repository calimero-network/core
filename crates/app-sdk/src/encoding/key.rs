//! Order-preserving tuple key encoding.
//!
//! Each part is a type tag byte then its payload, so the byte order of two encoded tuples
//! equals the tuple order: none < bool < int < string < bytes, then by value.

/// One element of a key tuple.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeyPart {
    /// No value.
    None,
    /// A boolean (`false < true`).
    Bool(bool),
    /// An integer of up to 128 bits.
    Int(i128),
    /// A string (compared bytewise as UTF-8).
    Str(String),
    /// Raw bytes.
    Bytes(Vec<u8>),
}

const TAG_NONE: u8 = 0x00;
const TAG_BOOL: u8 = 0x10;
const TAG_INT: u8 = 0x20;
const TAG_STR: u8 = 0x30;
const TAG_BYTES: u8 = 0x40;

/// Encode a tuple of parts.
pub fn encode(parts: &[KeyPart]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        encode_part(part, &mut out);
    }
    out
}

/// Append one part to `out`.
pub fn encode_part(part: &KeyPart, out: &mut Vec<u8>) {
    match part {
        KeyPart::None => out.push(TAG_NONE),
        KeyPart::Bool(b) => {
            out.push(TAG_BOOL);
            out.push(u8::from(*b));
        }
        KeyPart::Int(i) => {
            out.push(TAG_INT);
            // Flipping the sign bit makes two's complement sort as unsigned.
            let flipped = (*i as u128) ^ (1u128 << 127);
            out.extend_from_slice(&flipped.to_be_bytes());
        }
        KeyPart::Str(s) => {
            out.push(TAG_STR);
            escaped(s.as_bytes(), out);
        }
        KeyPart::Bytes(b) => {
            out.push(TAG_BYTES);
            escaped(b, out);
        }
    }
}

/// `0x00` becomes `0x00 0xFF`; the payload ends with `0x00 0x00`.
fn escaped(bytes: &[u8], out: &mut Vec<u8>) {
    for &b in bytes {
        out.push(b);
        if b == 0 {
            out.push(0xFF);
        }
    }
    out.extend_from_slice(&[0x00, 0x00]);
}

/// A value that is one key part.
pub trait ToKeyPart {
    /// This value as a key part.
    fn to_key_part(&self) -> KeyPart;
}

impl ToKeyPart for KeyPart {
    fn to_key_part(&self) -> KeyPart {
        self.clone()
    }
}

impl ToKeyPart for str {
    fn to_key_part(&self) -> KeyPart {
        KeyPart::Str(self.to_owned())
    }
}

impl ToKeyPart for String {
    fn to_key_part(&self) -> KeyPart {
        KeyPart::Str(self.clone())
    }
}

impl ToKeyPart for bool {
    fn to_key_part(&self) -> KeyPart {
        KeyPart::Bool(*self)
    }
}

impl ToKeyPart for [u8] {
    fn to_key_part(&self) -> KeyPart {
        KeyPart::Bytes(self.to_vec())
    }
}

impl ToKeyPart for Vec<u8> {
    fn to_key_part(&self) -> KeyPart {
        KeyPart::Bytes(self.clone())
    }
}

impl ToKeyPart for () {
    fn to_key_part(&self) -> KeyPart {
        KeyPart::None
    }
}

impl<T: ToKeyPart + ?Sized> ToKeyPart for &T {
    fn to_key_part(&self) -> KeyPart {
        (**self).to_key_part()
    }
}

impl<T: ToKeyPart> ToKeyPart for Option<T> {
    fn to_key_part(&self) -> KeyPart {
        self.as_ref().map_or(KeyPart::None, ToKeyPart::to_key_part)
    }
}

macro_rules! int_key_part {
    ($($t:ty),*) => {$(
        impl ToKeyPart for $t {
            fn to_key_part(&self) -> KeyPart {
                KeyPart::Int(i128::from(*self))
            }
        }
    )*};
}

int_key_part!(u8, u16, u32, u64, i8, i16, i32, i64, i128);

/// A value that is a whole key tuple: one part, a tuple of parts or a slice of parts.
pub trait ToKey {
    /// This value as key parts.
    fn to_key(&self) -> Vec<KeyPart>;
}

impl<T: ToKeyPart + ?Sized> ToKey for T {
    fn to_key(&self) -> Vec<KeyPart> {
        vec![self.to_key_part()]
    }
}

impl ToKey for [KeyPart] {
    fn to_key(&self) -> Vec<KeyPart> {
        self.to_vec()
    }
}

impl ToKey for Vec<KeyPart> {
    fn to_key(&self) -> Vec<KeyPart> {
        self.clone()
    }
}

macro_rules! tuple_key {
    ($($name:ident),+) => {
        impl<$($name: ToKeyPart),+> ToKey for ($($name,)+) {
            #[allow(non_snake_case)]
            fn to_key(&self) -> Vec<KeyPart> {
                let ($($name,)+) = self;
                vec![$($name.to_key_part()),+]
            }
        }
    };
}

tuple_key!(A);
tuple_key!(A, B);
tuple_key!(A, B, C);
tuple_key!(A, B, C, D);
