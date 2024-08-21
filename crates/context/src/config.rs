use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApplicationConfig {
    pub dir: camino::Utf8PathBuf,
}
