use reqwest::Url;

pub(crate) fn get_ip(
    multiaddr: &libp2p::Multiaddr,
    api_path: Option<String>,
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

    let mut url = Url::parse(&format!("http://{}:{}", ip, port))?;

    if let Some(api_path) = api_path {
        url.set_path(&api_path);
    }
    Ok(url)
}
