//! serde helpers for JSON events and error fields: 64-bit integers are decimal strings (D20).
//!
//! Use as `#[serde(with = "u64_str")]` (and `i64_str`, `opt_u64_str`, `opt_i64_str`); the
//! `#[app::event]` and `#[app::error]` macros apply them to `u64`/`i64` fields. Reading accepts
//! only strings, so a JS number that lost precision is refused instead of misread.

use core::fmt::Display;
use core::str::FromStr;

use serde::de::Error;
use serde::{Deserialize, Deserializer, Serializer};

fn ser<T: Display, S: Serializer>(v: &T, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(v)
}

fn de<'de, T: FromStr, D: Deserializer<'de>>(d: D) -> Result<T, D::Error>
where
    T::Err: Display,
{
    String::deserialize(d)?.parse().map_err(D::Error::custom)
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
                Option::<String>::deserialize(d)?
                    .map(|v| v.parse().map_err(D::Error::custom))
                    .transpose()
            }
        }
    };
}

str_int!(u64_str, opt_u64_str, u64);
str_int!(i64_str, opt_i64_str, i64);
