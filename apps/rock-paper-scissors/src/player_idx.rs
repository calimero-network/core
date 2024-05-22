use std::ops::Deref;

use calimero_sdk::serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Copy, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub(crate) struct PlayerIdx(usize);

impl Deref for PlayerIdx {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PlayerIdx {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Deserialize::deserialize(deserializer)?;
        match value {
            0 | 1 => Ok(PlayerIdx(value)),
            _ => Err(calimero_sdk::serde::de::Error::custom(
                "Player index must be 0 or 1",
            )),
        }
    }
}
