use calimero_context_config::config::ContextConfigConfig;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfig {
    pub config: ContextConfigConfig,
}
