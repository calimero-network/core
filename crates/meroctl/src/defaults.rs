pub(crate) const DEFAULT_CALIMERO_HOME: &str = "data";

pub fn default_chat_dir() -> camino::Utf8PathBuf {
    if let Some(home) = dirs::home_dir() {
        let home = camino::Utf8Path::from_path(&home).expect("invalid home directory");
        return home.join(DEFAULT_CALIMERO_HOME);
    }

    Default::default()
}
