use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ApplicationConfig {
    pub dir: camino::Utf8PathBuf,
}
