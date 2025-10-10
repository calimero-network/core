use std::convert::Infallible;

use generic_array::typenum::U32;
use libp2p::kad::RecordKey;

use crate::key::{component::KeyComponent, AsKeyParts, FromKeyParts, Key};
#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Clone, Copy, Debug)]
pub struct RecordID;

impl KeyComponent for RecordID {
    type LEN = U32;
    // Clueless on what will be the len as it is dynamic
}

macro_rules! create_kad_meta {
    ($name:ident,$column:expr) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
        pub struct $name(Key<RecordID>);

        impl $name {
            pub const SHA_256_MH: u64 = 18;

            #[must_use]
            pub fn new(record_key: &RecordKey) -> Self {
                let val = record_key.to_vec();
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&val[val.len() - 32..]);

                Self(Key(hash.into()))
            }

            #[must_use]
            pub fn record(&self) -> RecordKey {
                let hash: [u8; 32] = *AsRef::<[_; 32]>::as_ref(&self.0);
                RecordKey::from(
                    libp2p::multihash::Multihash::<64>::wrap(Self::SHA_256_MH, &hash).unwrap(),
                )
            }
        }

        impl AsKeyParts for $name {
            type Components = (RecordID,);

            fn column() -> crate::db::Column {
                $column
            }

            fn as_key(&self) -> &Key<Self::Components> {
                (&self.0).into()
            }
        }

        impl FromKeyParts for $name {
            type Error = Infallible;

            fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
                Ok(Self(*<&_>::from(&parts)))
            }
        }
    };
}

create_kad_meta!(RecordMeta, crate::db::Column::KadRecord);

create_kad_meta!(ProviderRecordMeta, crate::db::Column::KadProviderRecord);

#[cfg(test)]
mod tests {
    use libp2p::kad::RecordKey;
    use libp2p::multihash;
    use rand;
    use rand::Rng;

    use crate::key::RecordMeta;

    #[test]
    fn test_key() {
        let hash = rand::thread_rng().gen::<[u8; 32]>();
        let key: multihash::Multihash<64> =
            multihash::Multihash::wrap(RecordMeta::SHA_256_MH, &hash).unwrap();

        let key = RecordKey::from(key);

        // Retrieve back key
        let compiled = key.to_vec();
        assert!(compiled.len() > 32);

        let mut new_hash = [0u8; 32];
        new_hash.copy_from_slice(&compiled[compiled.len() - 32..]);
        assert_eq!(hash, new_hash)
    }
}
