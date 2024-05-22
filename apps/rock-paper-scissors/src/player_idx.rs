use calimero_sdk::serde::{Deserialize, Deserializer, Serialize};

#[derive(Copy, Clone, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct PlayerIdx(pub usize);

impl PlayerIdx {
    pub fn other(&self) -> PlayerIdx {
        PlayerIdx(1 - self.0)
    }

    pub fn is_first(&self) -> bool {
        self.0 == 0
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
