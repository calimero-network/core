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

fn get_docker_host_url(port: u16) -> String {
    // Check if we're in Docker
    if !std::path::Path::new("/.dockerenv").exists() {
        return format!("http://localhost:{}", port);
    }

    // Use host.docker.internal (works on Mac/Windows Docker Desktop)
    // On Linux, fall back to default Docker bridge gateway
    if cfg!(target_os = "linux") {
        format!("http://172.17.0.1:{}", port)
    } else {
        format!("http://host.docker.internal:{}", port)
    }
}

pub fn default_relayer_url() -> Url {
    let url = get_docker_host_url(63529);
    url.parse().expect("invalid default relayer URL")
}
