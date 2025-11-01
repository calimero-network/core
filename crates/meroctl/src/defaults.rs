use camino::{Utf8Path, Utf8PathBuf};

pub const DEFAULT_CALIMERO_HOME: &str = ".calimero";

/// Get default node directory
///
/// Desktop mode: Uses platform-appropriate directories
/// Server mode: Uses ~/.calimero for backwards compatibility
pub fn default_node_dir() -> Utf8PathBuf {
    #[cfg(not(feature = "use-home-dir"))]
    {
        use calimero_config::dirs::resolve_app_dirs;

        if let Ok(dirs) = resolve_app_dirs("Calimero") {
            if let Ok(path) = Utf8PathBuf::from_path_buf(dirs.data) {
                return path;
            }
        }
        return Utf8PathBuf::from(DEFAULT_CALIMERO_HOME);
    }

    #[cfg(feature = "use-home-dir")]
    {
        if let Some(home) = dirs::home_dir() {
            if let Some(home_utf8) = Utf8Path::from_path(&home) {
                return home_utf8.join(DEFAULT_CALIMERO_HOME);
            }
        }
        Utf8PathBuf::from(DEFAULT_CALIMERO_HOME)
    }
}
