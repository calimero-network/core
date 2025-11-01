use camino::{Utf8Path, Utf8PathBuf};
use url::Url;

pub const DEFAULT_CALIMERO_HOME: &str = ".calimero";
pub const DEFAULT_RELAYER_URL: &str = "http://3.125.79.112:63529";

/// Get default node directory
///
/// Desktop mode: Uses platform-appropriate directories
/// Server mode: Uses ~/.calimero for backwards compatibility
pub fn default_node_dir() -> Utf8PathBuf {
    #[cfg(not(feature = "use-home-dir"))]
    {
        use calimero_config::dirs::resolve_app_dirs;

        match resolve_app_dirs("Calimero") {
            Ok(dirs) => {
                // Convert PathBuf to Utf8PathBuf
                match Utf8PathBuf::from_path_buf(dirs.data) {
                    Ok(path) => path,
                    Err(_) => {
                        // Fallback if path contains invalid UTF-8
                        Utf8PathBuf::from(DEFAULT_CALIMERO_HOME)
                    }
                }
            }
            Err(_) => {
                // Fallback if directories crate fails
                Utf8PathBuf::from(DEFAULT_CALIMERO_HOME)
            }
        }
    }

    #[cfg(feature = "use-home-dir")]
    {
        // Server mode: legacy behavior
        use dirs::home_dir;

        if let Some(home) = home_dir() {
            if let Some(home_utf8) = Utf8Path::from_path(&home) {
                return home_utf8.join(DEFAULT_CALIMERO_HOME);
            }
        }

        Utf8PathBuf::from(DEFAULT_CALIMERO_HOME)
    }
}

pub fn default_relayer_url() -> Url {
    DEFAULT_RELAYER_URL
        .parse()
        .expect("invalid default relayer URL")
}
