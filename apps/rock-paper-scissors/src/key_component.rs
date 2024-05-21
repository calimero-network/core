use std::io;

use bs58::decode::DecodeTarget;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{de, ser, Deserialize, Serialize};

use crate::keys::KeyBytes;

#[derive(Debug, Default)]
pub struct KeyComponent<T: KeyBytes> {
    pub(crate) key: T,
}

impl<T: KeyBytes> From<T> for KeyComponent<T> {
    fn from(key: T) -> Self {
        KeyComponent { key }
    }
}

impl<T: KeyBytes> Serialize for KeyComponent<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        let bytes = self.key.to_bytes();
        let encoded = bs58::encode(bytes).into_string();
        serializer.serialize_str(&encoded)
    }
}

impl<'de, T: KeyBytes> Deserialize<'de> for KeyComponent<T>
where
    T::Bytes: Default + DecodeTarget,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let encoded = <String as de::Deserialize>::deserialize(deserializer)?;

        let mut bytes = T::Bytes::default();

        bs58::decode(&encoded)
            .onto(&mut bytes)
            .map_err(de::Error::custom)?;

        let key = T::from_bytes(bytes).map_err(|_| de::Error::custom("invalid key"))?;

        Ok(KeyComponent { key })
    }
}

impl<T: KeyBytes> BorshSerialize for KeyComponent<T>
where
    T::Bytes: BorshSerialize,
{
    fn serialize<W: io::Write>(&self, writer: &mut W) -> Result<(), io::Error> {
        self.key.to_bytes().serialize(writer)
    }
}

impl<T: KeyBytes> BorshDeserialize for KeyComponent<T>
where
    T::Bytes: BorshDeserialize,
{
    fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
        let key_bytes = T::Bytes::deserialize_reader(reader)?;

        let key = T::from_bytes(key_bytes).map_err(|_| io::ErrorKind::InvalidData)?;

        Ok(KeyComponent { key })
    }
}
