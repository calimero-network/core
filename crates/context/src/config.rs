use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplicationConfig {
    pub dir: camino::Utf8PathBuf,
}
