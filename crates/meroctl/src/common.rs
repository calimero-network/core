use calimero_config::ConfigFile;
use camino::Utf8PathBuf;
use chrono::Utc;
use eyre::{eyre, Error as EyreError};
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use reqwest::{Client, Response, Url};
use serde::{Deserialize, Serialize};
use serde_json::{to_value, Value};

pub fn multiaddr_to_url(multiaddr: &Multiaddr, api_path: &str) -> Result<Url, CliError<EyreError>> {
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

    let ip =
        ip.ok_or_else(|| CliError::InternalError(eyre!("No IP address found in Multiaddr")))?;
    let port =
        port.ok_or_else(|| CliError::InternalError(eyre!("No TCP port found in Multiaddr")))?;
    let scheme = scheme.unwrap_or("http");

    let mut url = Url::parse(&format!("{scheme}://{ip}:{port}"))
        .map_err(|_| CliError::InternalError(eyre!("Couldn't parse url")))?;

    url.set_path(api_path);

    Ok(url)
}

pub async fn get_response<S>(
    client: &Client,
    url: Url,
    body: Option<S>,
    keypair: &Keypair,
    req_type: RequestType,
) -> Result<Response, CliError<EyreError>>
where
    S: Serialize,
{
    let timestamp = Utc::now().timestamp().to_string();
    let signature = keypair
        .sign(timestamp.as_bytes())
        .map_err(|_| CliError::InternalError(eyre!("Couldn't sign keypair")))?;

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
        .map_err(|_| CliError::InternalError(eyre!("Error with client request")))
}

pub fn load_config(path: &Utf8PathBuf) -> Result<ConfigFile, CliError<EyreError>> {
    if !ConfigFile::exists(&path) {
        return Err(CliError::InternalError(eyre!("Config file does not exist")));
    };

    let Ok(config) = ConfigFile::load(&path) else {
        return Err(CliError::InternalError(eyre!("Failed to load config file")));
    };

    Ok(config)
}

pub fn fetch_multiaddr(config: &ConfigFile) -> Result<&Multiaddr, CliError<EyreError>> {
    let Some(multiaddr) = config.network.server.listen.first() else {
        return Err(CliError::InternalError(eyre!("No address found")));
    };

    Ok(multiaddr)
}

pub enum RequestType {
    Get,
    Post,
    Delete,
}

#[derive(Debug)]
pub enum CliError<E> {
    MethodCallError(E),
    InternalError(EyreError),
}

pub trait ToResponseBody {
    fn to_res_body(self) -> ResponseBody;
}

impl<T: Serialize, E: Serialize> ToResponseBody for Result<T, CliError<E>> {
    fn to_res_body(self) -> ResponseBody {
        match self {
            Ok(r) => match to_value(r) {
                Ok(v) => ResponseBody::Result(v),
                Err(e) => ResponseBody::Error(ResponseBodyError::ServerError(e.into())),
            },
            Err(CliError::MethodCallError(err)) => match to_value(err) {
                Ok(v) => ResponseBody::Error(ResponseBodyError::HandlerError(v)),
                Err(e) => ResponseBody::Error(ResponseBodyError::ServerError(e.into())),
            },
            Err(CliError::InternalError(err)) => {
                ResponseBody::Error(ResponseBodyError::ServerError(err))
            }
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::exhaustive_enums,
    reason = "This will never have any other variants"
)]
pub enum ResponseBody {
    Result(Value),
    #[serde(skip)]
    Error(ResponseBodyError),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ResponseBodyError {
    HandlerError(Value),
    #[serde(skip)]
    ServerError(EyreError),
}
