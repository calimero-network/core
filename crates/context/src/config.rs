use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplicationConfig {
    pub dir: camino::Utf8PathBuf,

    pub cathup: CatchupConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatchupConfig {
    pub batch_size: u8,
}
