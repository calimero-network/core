use serde::{Deserialize, Serialize};

#[derive(Eq, Hash, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApplicationId(pub String);
