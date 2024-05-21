use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Deserializer, Serialize};

use crate::keys::AsKeyBytes;

#[derive(Default, Debug, PartialEq)]
pub struct KeyComponent<T: AsKeyBytes> {
    pub(crate) key: T,
}

impl<T: AsKeyBytes> From<T> for KeyComponent<T> {
    fn from(key: T) -> Self {
        KeyComponent { key }
    }
}

impl<T: AsKeyBytes> Serialize for KeyComponent<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: calimero_sdk::serde::Serializer,
    {
        let bytes = self.key.as_key_bytes();
        let encoded = bs58::encode(bytes).into_string();
        serializer.serialize_str(&encoded)
    }
}

impl<'de, T: AsKeyBytes> Deserialize<'de> for KeyComponent<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = <std::string::String as Deserialize>::deserialize(deserializer)?;
        let bytes_decoded = bs58::decode(&encoded)
            .into_vec()
            .map_err(calimero_sdk::serde::de::Error::custom)?;
        let key =
            T::from_key_bytes(&bytes_decoded).map_err(calimero_sdk::serde::de::Error::custom)?;
        Ok(KeyComponent { key })
    }
}

impl<T: AsKeyBytes> BorshSerialize for KeyComponent<T> {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> Result<(), std::io::Error> {
        let binding = self.key.as_key_bytes();
        let bytes = binding.as_ref();
        writer.write_all(&bytes)?;
        Ok(())
    }
}

impl<T: AsKeyBytes> BorshDeserialize for KeyComponent<T> {
    fn deserialize_reader<R: std::io::prelude::Read>(reader: &mut R) -> std::io::Result<Self> {
        let mut key_bytes = vec![];
        reader.read(&mut key_bytes)?;

        let key = T::from_key_bytes(&key_bytes).map_err(|_| std::io::ErrorKind::OutOfMemory)?;

        Ok(KeyComponent { key })
    }
}
