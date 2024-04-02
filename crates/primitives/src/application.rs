use serde::{Deserialize, Serialize};

#[derive(Eq, Hash, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApplicationId(pub String);

impl ApplicationId {
    pub fn to_string(self) -> String {
        self.0
    }
}
