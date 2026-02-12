use camino::{Utf8Path, Utf8PathBuf};
use dirs::home_dir;
use url::Url;

use crate::docker;

pub const DEFAULT_CALIMERO_HOME: &str = ".calimero";
pub const DEFAULT_RELAYER_URL: &str = "http://63.179.161.75:63529";
const LOCAL_RELAYER_PORT: u16 = 63529;

pub fn default_node_dir() -> Utf8PathBuf {
    if let Some(home) = home_dir() {
        let home = Utf8Path::from_path(&home).expect("invalid home directory");
        return home.join(DEFAULT_CALIMERO_HOME);
    }

    Utf8PathBuf::default()
}

/// Get the default relayer URL for NEAR operations.
pub fn default_relayer_url() -> Url {
    let selected = if std::path::Path::new("/.dockerenv").exists() {
        docker::get_docker_host_for_port(LOCAL_RELAYER_PORT)
    } else {
        DEFAULT_RELAYER_URL.to_owned()
    };

    selected.parse().expect("invalid default relayer URL")
}
