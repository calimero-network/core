use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Deserializer, Serialize};

use crate::keys::KeyBytes;

#[derive(Default, Debug, PartialEq)]
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
        S: calimero_sdk::serde::Serializer,
    {
        let bytes = self.key.as_key_bytes();
        let encoded = bs58::encode(bytes).into_string();
        serializer.serialize_str(&encoded)
    }
}

impl<'de, T: KeyBytes> Deserialize<'de> for KeyComponent<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = <std::string::String as Deserialize>::deserialize(deserializer)?;
        let bytes_decoded = bs58::decode(&encoded)
            .into_vec()
            .map_err(calimero_sdk::serde::de::Error::custom)?;

        let key = match bytes_decoded.len() {
            32 => {
                let mut array = [0u8; 32];
                array.copy_from_slice(&bytes_decoded);
                T::from_key_bytes(array).map_err(calimero_sdk::serde::de::Error::custom(
                    "Invalid byte length",
                ))?
            }
            64 => {
                let mut array = [0u8; 64];
                array.copy_from_slice(&bytes_decoded);
                T::from_key_bytes(array).map_err(calimero_sdk::serde::de::Error::custom(
                    "Invalid byte length",
                ))?
            }
            _ => {
                return Err(calimero_sdk::serde::de::Error::custom(
                    "Invalid byte length",
                ))
            }
        };

        Ok(KeyComponent { key })
    }
}

impl<T: KeyBytes> BorshSerialize for KeyComponent<T> {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> Result<(), std::io::Error> {
        let binding = self.key.as_key_bytes();
        let bytes = binding.as_ref();
        writer.write_all(&bytes)?;
        Ok(())
    }
}

impl<T: KeyBytes> BorshDeserialize for KeyComponent<T>
where
    <T as KeyBytes>::Bytes: BorshDeserialize,
{
    fn deserialize_reader<R: std::io::prelude::Read>(reader: &mut R) -> std::io::Result<Self> {
        let key_bytes = <T::Bytes as BorshDeserialize>::deserialize_reader(reader)?;

        let key = T::from_key_bytes(key_bytes).map_err(|_| std::io::ErrorKind::InvalidData)?;

        Ok(KeyComponent { key })
    }
}
