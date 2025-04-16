use eyre::{eyre, Result as EyreResult};
use multiaddr::{Multiaddr, Protocol};
use serde::{Deserialize, Serialize};
use url::Url;

#[must_use]
pub const fn bool_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(remote = "Result")]
#[expect(clippy::exhaustive_enums, reason = "This cannot have more variants")]
pub enum ResultAlt<T, E> {
    #[serde(rename = "result")]
    Ok(T),
    #[serde(rename = "error")]
    Err(E),
}

impl<T, E> From<ResultAlt<T, E>> for Result<T, E> {
    fn from(result: ResultAlt<T, E>) -> Self {
        match result {
            ResultAlt::Ok(value) => Ok(value),
            ResultAlt::Err(err) => Err(err),
        }
    }
}

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
