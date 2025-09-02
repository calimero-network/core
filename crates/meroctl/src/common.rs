use calimero_config::ConfigFile;
use camino::Utf8Path;
use eyre::{bail, eyre, Result, WrapErr};
use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use reqwest::Url;

pub fn multiaddr_to_url(multiaddr: &Multiaddr, api_path: &str) -> Result<Url> {
    #[expect(clippy::wildcard_enum_match_arm, reason = "Acceptable here")]
    let (ip, port, scheme) = multiaddr.iter().fold(
        (None, None, None),
        |(ip, port, scheme), protocol| match protocol {
            Protocol::Ip4(addr) => (Some(addr), port, scheme),
            Protocol::Tcp(p) => (ip, Some(p), scheme),
            Protocol::Http => (ip, port, Some("http")),
            Protocol::Https => (ip, port, Some("https")),
            _ => (ip, port, scheme),
        },
    );

    let ip = ip.ok_or_else(|| eyre!("No IP address found in Multiaddr"))?;
    let port = port.ok_or_else(|| eyre!("No TCP port found in Multiaddr"))?;
    let scheme = scheme.unwrap_or("http");

    let mut url = Url::parse(&format!("{scheme}://{ip}:{port}"))?;

    // Only set path if api_path is not empty, otherwise set to empty string to avoid trailing slash
    if !api_path.is_empty() {
        url.set_path(api_path);
    } else {
        url.set_path("");
    }

    Ok(url)
}

pub async fn load_config(home: &Utf8Path, node_name: &str) -> Result<ConfigFile> {
    let path = home.join(node_name);

    if !ConfigFile::exists(&path) {
        bail!("Config file does not exist");
    }

    let config = ConfigFile::load(&path)
        .await
        .wrap_err("Failed to load config file")?;

    Ok(config)
}

pub fn fetch_multiaddr(config: &ConfigFile) -> Result<&Multiaddr> {
    let Some(multiaddr) = config.network.server.listen.first() else {
        bail!("No address.")
    };

    Ok(multiaddr)
}
