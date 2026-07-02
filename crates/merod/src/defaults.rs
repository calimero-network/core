use camino::{Utf8Path, Utf8PathBuf};
use dirs::home_dir;

pub const DEFAULT_CALIMERO_HOME: &str = ".calimero";

pub fn default_node_dir() -> Utf8PathBuf {
    // A non-UTF-8 home directory is unusable here (paths are `Utf8PathBuf`), but
    // it must not abort the process — fall back to a relative default and let the
    // caller surface a clear error (or accept an explicit `--home`).
    if let Some(home) = home_dir() {
        match Utf8Path::from_path(&home) {
            Some(home) => return home.join(DEFAULT_CALIMERO_HOME),
            None => tracing::warn!(
                home = ?home,
                "home directory is not valid UTF-8; falling back to a relative default \
                 (pass --home to set an explicit path)"
            ),
        }
    }

    Utf8PathBuf::default()
}
