use camino::{Utf8Path, Utf8PathBuf};
use dirs::home_dir;
use url::Url;

pub const DEFAULT_CALIMERO_HOME: &str = ".calimero";
pub const DEFAULT_RELAYER_URL: &str = "http://63.179.161.75:63529";

pub fn default_node_dir() -> Utf8PathBuf {
    if let Some(home) = home_dir() {
        let home = Utf8Path::from_path(&home).expect("invalid home directory");
        return home.join(DEFAULT_CALIMERO_HOME);
    }

    Utf8PathBuf::default()
}

pub fn default_relayer_url() -> Url {
    DEFAULT_RELAYER_URL
        .parse()
        .expect("invalid default relayer URL")
}
