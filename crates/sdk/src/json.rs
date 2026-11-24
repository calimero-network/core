//! serde helpers for JSON (D20): 64-bit integers are decimal strings and bytes are base64url.
//!
//! Use as `#[serde(with = "u64_str")]` (and `i64_str`, `opt_u64_str`, `opt_i64_str`,
//! `bytes_b64url`, `opt_bytes_b64url`); the `app` macros apply them to method arguments and
//! results and to event and error fields. Reading accepts only strings, so a JS number that
//! lost precision is refused instead of misread, and only canonical decimals: no sign `+`,
//! no leading zeros, no `-0`. The `*_auto` forms are for record fields: the JSON form in
//! human-readable formats, the native CBOR form (integer, byte string) otherwise.

use core::fmt::{self, Display};
use core::marker::PhantomData;
use core::str::FromStr;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

fn ser<T: Display, S: Serializer>(v: &T, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(v)
}

/// A canonical decimal: digits, an optional `-`, no leading zeros, no `-0`.
fn parse_decimal<T: FromStr, E: Error>(text: &str) -> Result<T, E>
where
    T::Err: Display,
{
    let digits = text.strip_prefix('-').unwrap_or(text);
    let canonical = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
        && text != "-0";
    if !canonical {
        return Err(E::custom(format!("{text:?} is not a canonical decimal")));
    }
    text.parse().map_err(E::custom)
}

/// A 64-bit integer read from its decimal string; a JSON number is refused as "expected a
/// u64 as a decimal string", so the caller learns the wire form.
struct Dec<T>(T);

impl<'de, T: FromStr> Deserialize<'de> for Dec<T>
where
    T::Err: Display,
{
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct DecVisitor<T>(PhantomData<T>);

        impl<T: FromStr> Visitor<'_> for DecVisitor<T>
        where
            T::Err: Display,
        {
            type Value = T;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a {} as a decimal string", core::any::type_name::<T>())
            }

            fn visit_str<E: Error>(self, v: &str) -> Result<T, E> {
                parse_decimal(v)
            }
        }

        d.deserialize_str(DecVisitor(PhantomData)).map(Dec)
    }
}

fn de<'de, T: FromStr, D: Deserializer<'de>>(d: D) -> Result<T, D::Error>
where
    T::Err: Display,
{
    Dec::deserialize(d).map(|Dec(v)| v)
}

fn de_opt<'de, T: FromStr, D: Deserializer<'de>>(d: D) -> Result<Option<T>, D::Error>
where
    T::Err: Display,
{
    Ok(Option::<Dec<T>>::deserialize(d)?.map(|Dec(v)| v))
}

macro_rules! str_int {
    ($mod:ident, $opt:ident, $t:ty) => {
        /// A `$t` as a decimal string.
        pub mod $mod {
            use super::*;

            /// Serialize as a decimal string.
            pub fn serialize<S: Serializer>(v: &$t, s: S) -> Result<S::Ok, S::Error> {
                ser(v, s)
            }

            /// Deserialize from a decimal string.
            pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<$t, D::Error> {
                de(d)
            }
        }

        /// An optional `$t` as a decimal string or null.
        pub mod $opt {
            use super::*;

            /// Serialize as a decimal string, or null.
            pub fn serialize<S: Serializer>(v: &Option<$t>, s: S) -> Result<S::Ok, S::Error> {
                match v {
                    Some(v) => ser(v, s),
                    None => s.serialize_none(),
                }
            }

            /// Deserialize from a decimal string, or null.
            pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<$t>, D::Error> {
                de_opt(d)
            }
        }
    };
}

str_int!(u64_str, opt_u64_str, u64);
str_int!(i64_str, opt_i64_str, i64);

macro_rules! auto_int {
    ($mod:ident, $opt:ident, $t:ty) => {
        /// A `$t` record field: a decimal string in human-readable formats (JSON results), an
        /// integer otherwise (stored CBOR). Applied by `#[derive(Record)]`.
        pub mod $mod {
            use super::*;

            /// A string for JSON, an integer for CBOR.
            pub fn serialize<S: Serializer>(v: &$t, s: S) -> Result<S::Ok, S::Error> {
                if s.is_human_readable() {
                    ser(v, s)
                } else {
                    serde::Serialize::serialize(v, s)
                }
            }

            /// A string from JSON, an integer from CBOR.
            pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<$t, D::Error> {
                if d.is_human_readable() {
                    de(d)
                } else {
                    <$t>::deserialize(d)
                }
            }
        }

        /// An optional `$t` record field, as the plain one; `None` is null in both.
        pub mod $opt {
            use super::*;

            /// A string or null for JSON, an integer or null for CBOR.
            pub fn serialize<S: Serializer>(v: &Option<$t>, s: S) -> Result<S::Ok, S::Error> {
                match v {
                    Some(v) => $mod::serialize(v, s),
                    None => s.serialize_none(),
                }
            }

            /// A string or null from JSON, an integer or null from CBOR.
            pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<$t>, D::Error> {
                if d.is_human_readable() {
                    de_opt(d)
                } else {
                    Option::<$t>::deserialize(d)
                }
            }
        }
    };
}

auto_int!(u64_auto, opt_u64_auto, u64);
auto_int!(i64_auto, opt_i64_auto, i64);

fn ser_b64<T: AsRef<[u8]> + ?Sized, S: Serializer>(v: &T, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&URL_SAFE_NO_PAD.encode(v))
}

fn de_b64<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    URL_SAFE_NO_PAD
        .decode(String::deserialize(d)?)
        .map_err(D::Error::custom)
}

/// Bytes as base64url without padding.
pub mod bytes_b64url {
    use super::*;

    /// Serialize as base64url.
    pub fn serialize<T: AsRef<[u8]> + ?Sized, S: Serializer>(
        v: &T,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        ser_b64(v, s)
    }

    /// Deserialize from base64url; anything else is an error.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        de_b64(d)
    }
}

/// Optional bytes as base64url or null.
pub mod opt_bytes_b64url {
    use super::*;

    /// Serialize as base64url, or null.
    pub fn serialize<T: AsRef<[u8]>, S: Serializer>(
        v: &Option<T>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => ser_b64(v, s),
            None => s.serialize_none(),
        }
    }

    /// Deserialize from base64url, or null.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|v| URL_SAFE_NO_PAD.decode(v).map_err(D::Error::custom))
            .transpose()
    }
}

/// Reads a CBOR byte string (major type 2) into a `Vec<u8>`; arrays are refused.
struct ByteBuf;

impl<'de> Visitor<'de> for ByteBuf {
    type Value = Vec<u8>;

    fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("a byte string")
    }

    fn visit_bytes<E: Error>(self, v: &[u8]) -> Result<Vec<u8>, E> {
        Ok(v.to_vec())
    }

    fn visit_byte_buf<E: Error>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {
        Ok(v)
    }
}

/// A bytes record field: base64url in JSON, a CBOR byte string when stored.
pub mod bytes_auto {
    use super::*;

    /// base64url for JSON, a byte string for CBOR.
    pub fn serialize<T: AsRef<[u8]> + ?Sized, S: Serializer>(
        v: &T,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            ser_b64(v, s)
        } else {
            s.serialize_bytes(v.as_ref())
        }
    }

    /// base64url from JSON, a byte string from CBOR.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        if d.is_human_readable() {
            de_b64(d)
        } else {
            d.deserialize_byte_buf(ByteBuf)
        }
    }
}

/// An optional bytes record field, as [`bytes_auto`]; `None` is null in both.
pub mod opt_bytes_auto {
    use super::*;

    /// base64url or null for JSON, a byte string or null for CBOR.
    pub fn serialize<T: AsRef<[u8]>, S: Serializer>(
        v: &Option<T>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => bytes_auto::serialize(v, s),
            None => s.serialize_none(),
        }
    }

    /// base64url or null from JSON, a byte string or null from CBOR.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        #[derive(Deserialize)]
        #[serde(transparent)]
        struct Inner(#[serde(with = "bytes_auto")] Vec<u8>);
        Ok(Option::<Inner>::deserialize(d)?.map(|i| i.0))
    }
}

/// Bytes as a value in state or in a plain serde type: a CBOR byte string when stored (one
/// byte per byte), base64url in JSON. A bare `Vec<u8>` in a `Cell`, a map value or a plain
/// struct is stored as a CBOR list of numbers, about twice its size; records and method
/// parameters already convert `Vec<u8>` (D20), collections and plain structs need this.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bytes(pub Vec<u8>);

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        bytes_auto::serialize(&self.0, s)
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        bytes_auto::deserialize(d).map(Bytes)
    }
}

impl core::ops::Deref for Bytes {
    type Target = Vec<u8>;

    fn deref(&self) -> &Vec<u8> {
        &self.0
    }
}

impl core::ops::DerefMut for Bytes {
    fn deref_mut(&mut self) -> &mut Vec<u8> {
        &mut self.0
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(b: Bytes) -> Self {
        b.0
    }
}
