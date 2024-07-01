use reqwest::Url;

pub(crate) fn multiaddr_to_url(
    multiaddr: &libp2p::Multiaddr,
    api_path: &str,
) -> eyre::Result<reqwest::Url> {
    let ip = multiaddr
        .iter()
        .find_map(|p| match p {
            libp2p::multiaddr::Protocol::Ip4(ip) => Some(ip),
            _ => None,
        })
        .ok_or_else(|| eyre::eyre!("No IP address found in Multiaddr"))?;

    let port = multiaddr
        .iter()
        .find_map(|p| match p {
            libp2p::multiaddr::Protocol::Tcp(port) => Some(port),
            _ => None,
        })
        .ok_or_else(|| eyre::eyre!("No TCP port found in Multiaddr"))?;

    let scheme = multiaddr
        .iter()
        .find_map(|p| match p {
            libp2p::multiaddr::Protocol::Http => Some("http"),
            libp2p::multiaddr::Protocol::Https => Some("https"),
            _ => None,
        })
        .unwrap_or("http");

    let mut url = Url::parse(&format!("{}://{}:{}", scheme, ip, port))?;

    url.set_path(&api_path);

    Ok(url)
}
