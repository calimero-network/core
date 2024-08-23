use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ApplicationConfig {
    pub dir: Utf8PathBuf,
}

impl ApplicationConfig {
    #[must_use]
    pub const fn new(dir: Utf8PathBuf) -> Self {
        Self { dir }
    }
}
