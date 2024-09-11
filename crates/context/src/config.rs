use calimero_context_config::client::config::ContextConfigClientConfig;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfig {
    #[serde(rename = "config")]
    pub client: ContextConfigClientConfig,
}
