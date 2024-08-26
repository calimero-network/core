use chrono::Utc;
use eyre::{eyre, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use reqwest::{Client, Response, Url};
use serde::Serialize;

pub fn multiaddr_to_url(multiaddr: &Multiaddr, api_path: &str) -> EyreResult<Url> {
    #[allow(clippy::wildcard_enum_match_arm)]
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

pub async fn get_response<T: Serialize>(
    client: &Client,
    url: Url,
    request: Option<T>,
    keypair: &Keypair,
) -> EyreResult<Response> {
    let timestamp = Utc::now().timestamp().to_string();
    let signature = keypair.sign(timestamp.as_bytes())?;

    let mut builder = if request.is_some() {
        client.post(url).json(&request)
    } else {
        client.get(url)
    };

    builder = builder
        .header("X-Signature", hex::encode(signature))
        .header("X-Timestamp", timestamp);

    builder
        .send()
        .await
        .map_err(|_| eyre!("Error with client request"))
}
