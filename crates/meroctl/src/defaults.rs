use camino::{Utf8Path, Utf8PathBuf};
use dirs::home_dir;

pub const DEFAULT_CALIMERO_HOME: &str = ".calimero";

pub fn default_node_dir() -> Utf8PathBuf {
    if let Some(home) = home_dir() {
        let home = Utf8Path::from_path(&home).expect("invalid home directory");
        return home.join(DEFAULT_CALIMERO_HOME);
    }

    Utf8PathBuf::default()
}
