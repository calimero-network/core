use reqwest::Url;

pub(crate) fn multiaddr_to_url(
    multiaddr: &libp2p::Multiaddr,
    api_path: &str,
) -> eyre::Result<reqwest::Url> {
    let (ip, port, scheme) = multiaddr.iter().fold(
        (None, None, None),
        |(ip, port, scheme), protocol| match protocol {
            libp2p::multiaddr::Protocol::Ip4(addr) => (Some(addr), port, scheme),
            libp2p::multiaddr::Protocol::Tcp(p) => (ip, Some(p), scheme),
            libp2p::multiaddr::Protocol::Http => (ip, port, Some("http")),
            libp2p::multiaddr::Protocol::Https => (ip, port, Some("https")),
            _ => (ip, port, scheme),
        },
    );

    let ip = ip.ok_or_else(|| eyre::eyre!("No IP address found in Multiaddr"))?;
    let port = port.ok_or_else(|| eyre::eyre!("No TCP port found in Multiaddr"))?;
    let scheme = scheme.unwrap_or("http");

    let mut url = Url::parse(&format!("{}://{}:{}", scheme, ip, port))?;

    url.set_path(&api_path);

    Ok(url)
}
