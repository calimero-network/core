use camino::{Utf8Path, Utf8PathBuf};
use dirs::home_dir;
use url::Url;

use crate::cli::ConfigProtocol;
use crate::docker;

pub const DEFAULT_CALIMERO_HOME: &str = ".calimero";
pub const DEFAULT_RELAYER_URL: &str = "http://63.179.161.75:63529";

pub fn default_node_dir() -> Utf8PathBuf {
    if let Some(home) = home_dir() {
        let home = Utf8Path::from_path(&home).expect("invalid home directory");
        return home.join(DEFAULT_CALIMERO_HOME);
    }

    Utf8PathBuf::default()
}

/// Get the default relayer URL based on the protocol.
/// MockRelayer uses localhost detection ; all other protocols use the EC2 URL.
pub fn default_relayer_url(protocol: ConfigProtocol) -> Url {
    let url = if protocol == ConfigProtocol::MockRelayer {
        docker::get_docker_host_for_port(63529)
    } else {
        DEFAULT_RELAYER_URL.to_string()
    };
    url.parse().expect("invalid default relayer URL")
}
