use std::fmt::Display;

use serde::{Deserialize, Serialize};

#[derive(Eq, Hash, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApplicationId(pub String);

impl ApplicationId {
    pub fn to_string(self) -> String {
        self.0
    }
}

impl Display for ApplicationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl From<String> for ApplicationId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl Into<String> for ApplicationId {
    fn into(self) -> String {
        self.0
    }
}

impl AsRef<str> for ApplicationId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
