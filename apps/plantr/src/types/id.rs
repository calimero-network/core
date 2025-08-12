use core::fmt;
use core::marker::PhantomData;
use core::ops::Deref;
use core::str::{self, FromStr};
use std::borrow::Cow;

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{self, de, Deserialize, Serialize};

enum Dud<const N: usize> {}

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd, BorshDeserialize, BorshSerialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Id<const N: usize, const S: usize = 0> {
    bytes: [u8; N],
    _priv: PhantomData<Dud<S>>,
}

impl<const N: usize, const S: usize> Id<N, S> {
    #[doc(hidden)]
    pub const SIZE_GUARD: () = {
        let expected_size = (N + 1) * 4 / 3;
        let _guard = S - expected_size;
    };

    pub const fn new(id: [u8; N]) -> Self {
        let _guard = Self::SIZE_GUARD;

        Self {
            bytes: id,
            _priv: PhantomData,
        }
    }

    pub fn as_str<'a>(&self, buf: &'a mut [u8; S]) -> &'a str {
        let len = bs58::encode(&self.bytes)
            .onto(&mut buf[..])
            .expect("buffer too small");

        str::from_utf8(&buf[..len]).unwrap()
    }
}

impl<const N: usize, const S: usize> FromStr for Id<N, S> {
    type Err = bs58::decode::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut buf = [0; N];

        let _len = bs58::decode(s).onto(&mut buf[..])?;

        Ok(Self::new(buf))
    }
}

impl<const N: usize, const S: usize> AsRef<[u8]> for Id<N, S> {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl<const N: usize, const S: usize> AsRef<[u8; N]> for Id<N, S> {
    fn as_ref(&self) -> &[u8; N] {
        &self.bytes
    }
}

impl<const N: usize, const S: usize> Deref for Id<N, S> {
    type Target = [u8; N];

    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}

impl<const N: usize, const S: usize> From<[u8; N]> for Id<N, S> {
    fn from(id: [u8; N]) -> Self {
        Self::new(id)
    }
}

impl<const N: usize, const S: usize> fmt::Display for Id<N, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.pad(self.as_str(&mut [0; S]))
    }
}

impl<const N: usize, const S: usize> Serialize for Id<N, S> {
    fn serialize<O>(&self, serializer: O) -> Result<O::Ok, O::Error>
    where
        O: serde::Serializer,
    {
        let mut buf = [0; S];

        serializer.serialize_str(self.as_str(&mut buf))
    }
}

impl<'de, const N: usize, const S: usize> Deserialize<'de> for Id<N, S> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(crate = "calimero_sdk::serde")]
        struct Container<'a>(#[serde(borrow)] Cow<'a, str>);

        let encoded = Container::deserialize(deserializer)?;

        Self::from_str(&*encoded.0).map_err(de::Error::custom)
    }
}

#[doc(hidden)]
pub mod __private {
    pub use core::fmt;
    pub use core::ops::Deref;
    pub use core::prelude::v1::{
        AsRef, Clone, Copy, Debug, Eq, From, Ord, PartialEq, PartialOrd, Result,
    };
    pub use core::str::FromStr;

    pub use bs58;
    pub use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
    pub use calimero_sdk::serde::{Deserialize, Serialize};
}

macro_rules! define {
    ($name:ident < $len:literal $(, $str:literal )? >) => {
        $crate::types::id::define!(@ () $name < $len $(, $str )? >);
    };
    (pub $name:ident < $len:literal $(, $str:literal )?>) => {
        $crate::types::id::define!(@ (pub) $name < $len $(, $str )? >);
    };
    (@ ( $($vis:tt)* ) $name:ident < $len:literal $(, $str:literal )? >) => {
        #[derive(
            $crate::types::id::__private::Eq,
            $crate::types::id::__private::Ord,
            $crate::types::id::__private::Copy,
            $crate::types::id::__private::Clone,
            $crate::types::id::__private::Debug,
            $crate::types::id::__private::PartialEq,
            $crate::types::id::__private::PartialOrd,
            $crate::types::id::__private::Serialize,
            $crate::types::id::__private::Deserialize,
            $crate::types::id::__private::BorshSerialize,
            $crate::types::id::__private::BorshDeserialize,
        )]
        #[borsh(crate = "::calimero_sdk::borsh")]
        #[serde(crate = "::calimero_sdk::serde")]
        #[repr(transparent)]
        $($vis)* struct $name($crate::types::id::Id< $len $(, $str)? >);

        impl $name {
            pub const fn new(id: [u8; $len]) -> Self {
                Self::from_id($crate::types::id::Id::new(id))
            }

            const fn from_id(id: $crate::types::id::Id::< $len $(, $str)? >) -> Self {
                type DefinedId = $crate::types::id::Id::< $len $(, $str)? >;

                let _guard = DefinedId::SIZE_GUARD;

                Self(id)
            }
        }

        impl $crate::types::id::__private::Deref for $name {
            type Target = $crate::types::id::Id< $len $(, $str)? >;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl $crate::types::id::__private::AsRef<[u8]> for $name {
            fn as_ref(&self) -> &[u8] {
                $crate::types::id::__private::AsRef::as_ref(&self.0)
            }
        }

        impl $crate::types::id::__private::From<[u8; $len]> for $name {
            fn from(id: [u8; $len]) -> Self {
                Self::new(id)
            }
        }

        impl $crate::types::id::__private::fmt::Display for $name {
            fn fmt(
                &self,
                f: &mut $crate::types::id::__private::fmt::Formatter<'_>
            ) -> $crate::types::id::__private::fmt::Result {
                $crate::types::id::__private::fmt::Display::fmt(&self.0, f)
            }
        }

        impl $crate::types::id::__private::FromStr for $name {
            type Err = $crate::types::id::__private::bs58::decode::Error;

            fn from_str(s: &str) -> $crate::types::id::__private::Result<Self, Self::Err> {
                $crate::types::id::__private::Result::map(
                    $crate::types::id::__private::FromStr::from_str(s),
                    Self::from_id
                )
            }
        }
    };
}

pub(crate) use define;
