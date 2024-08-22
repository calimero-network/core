use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use reqwest::Url;

pub(crate) fn multiaddr_to_url(multiaddr: &Multiaddr, api_path: &str) -> eyre::Result<Url> {
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

    let ip = ip.ok_or_else(|| eyre::eyre!("No IP address found in Multiaddr"))?;
    let port = port.ok_or_else(|| eyre::eyre!("No TCP port found in Multiaddr"))?;
    let scheme = scheme.unwrap_or("http");

    let mut url = Url::parse(&format!("{scheme}://{ip}:{port}"))?;

    url.set_path(api_path);

    Ok(url)
}
