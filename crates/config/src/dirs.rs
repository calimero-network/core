//! Cross-platform directory resolution for Calimero
//!
//! Desktop mode: Uses platform-appropriate directories via `directories` crate
//! Server mode: Uses legacy ~/.calimero for backwards compatibility

use eyre::{eyre, Result};
use std::path::PathBuf;

/// Application directories for Calimero
#[derive(Debug, Clone)]
pub struct AppDirs {
    /// Configuration directory
    pub config: PathBuf,
    /// Data directory
    pub data: PathBuf,
    /// Logs directory
    pub logs: PathBuf,
}

/// Resolve platform-appropriate directories for the application
///
/// # Platform-specific paths
///
/// ## Desktop mode (no `use-home-dir` feature)
/// - **macOS:** `~/Library/Application Support/Calimero`
/// - **Windows:** `C:\Users\{user}\AppData\Roaming\Calimero`
/// - **Linux:** `~/.local/share/Calimero`
///
/// ## Server mode (with `use-home-dir` feature)
/// - **All platforms:** `~/.calimero` (backwards compatible)
///
/// # Arguments
/// * `app_name` - Application name (e.g., "Calimero")
///
/// # Errors
/// Returns error if directories cannot be determined
#[cfg(not(feature = "use-home-dir"))]
pub fn resolve_app_dirs(app_name: &str) -> Result<AppDirs> {
    use directories::ProjectDirs;

    let proj = ProjectDirs::from("network", "calimero", app_name)
        .ok_or_else(|| eyre!("Cannot resolve platform directories"))?;

    let data_dir = proj.data_dir().to_path_buf();

    Ok(AppDirs {
        config: proj.config_dir().to_path_buf(),
        data: data_dir.clone(),
        logs: data_dir.join("logs"),
    })
}

/// Resolve directories using legacy ~/.calimero path
///
/// This mode is for server deployments that expect the traditional
/// home directory structure.
#[cfg(feature = "use-home-dir")]
pub fn resolve_app_dirs(_app_name: &str) -> Result<AppDirs> {
    let home = dirs::home_dir().ok_or_else(|| eyre!("Cannot determine home directory"))?;

    let base = home.join(".calimero");

    Ok(AppDirs {
        config: base.clone(),
        data: base.clone(),
        logs: base.join("logs"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_app_dirs() {
        let result = resolve_app_dirs("CalimeroTest");
        assert!(result.is_ok(), "Should resolve directories");

        let dirs = result.unwrap();
        assert!(dirs.config.is_absolute(), "Config path should be absolute");
        assert!(dirs.data.is_absolute(), "Data path should be absolute");
        assert!(dirs.logs.is_absolute(), "Logs path should be absolute");
    }

    #[test]
    fn test_paths_are_different() {
        let dirs = resolve_app_dirs("CalimeroTest").unwrap();

        // Config and data might be same or different depending on platform
        // But logs should be under data
        assert!(
            dirs.logs.starts_with(&dirs.data),
            "Logs should be under data directory"
        );
    }
}
