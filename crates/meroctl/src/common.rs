use camino::Utf8PathBuf;
use chrono::Utc;
use eyre::{eyre, Result as EyreResult, bail};
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use reqwest::{Client, Response, Url};
use serde::Serialize;
use crate::config_file::ConfigFile;

pub fn multiaddr_to_url(multiaddr: &Multiaddr, api_path: &str) -> EyreResult<Url> {
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

    url.set_path(api_path);

    Ok(url)
}

pub async fn get_response<S>(
    client: &Client,
    url: Url,
    body: Option<S>,
    keypair: &Keypair,
    req_type: RequestType,
) -> EyreResult<Response>
where
    S: Serialize,
{
    let timestamp = Utc::now().timestamp().to_string();
    let signature = keypair.sign(timestamp.as_bytes())?;

    let mut builder = match req_type {
        RequestType::Get => client.get(url),
        RequestType::Post => client.post(url).json(&body),
        RequestType::Delete => client.delete(url),
    };

    builder = builder
        .header("X-Signature", bs58::encode(signature).into_string())
        .header("X-Timestamp", timestamp);

    builder
        .send()
        .await
        .map_err(|_| eyre!("Error with client request"))
}

pub fn load_config(path: &Utf8PathBuf) -> EyreResult<ConfigFile> {
    if !ConfigFile::exists(&path) {
        bail!("Config file does not exist")
    };

    let Ok(config) = ConfigFile::load(&path) else {
        bail!("Failed to load config file")
    };

    Ok(config)
}

pub fn load_multiaddr(config: &ConfigFile) -> EyreResult<&Multiaddr> {
    let Some(multiaddr) = config.network.server.listen.first() else {
        bail!("No address.")
    };

    Ok(multiaddr)
}

pub enum RequestType {
    Get,
    Post,
    Delete,
}
