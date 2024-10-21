use calimero_config::ConfigFile;
use camino::Utf8PathBuf;
use chrono::Utc;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use reqwest::{Client, Response, Url};
use serde::{Deserialize, Serialize};
use serde_json::{to_value, Value};

pub fn multiaddr_to_url(multiaddr: &Multiaddr, api_path: &str) -> Result<Url, CliError> {
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
        ip.ok_or_else(|| CliError::InternalError(format!("No IP address found in Multiaddr")))?;
    let port =
        port.ok_or_else(|| CliError::InternalError(format!("No TCP port found in Multiaddr")))?;
    let scheme = scheme.unwrap_or("http");

    let mut url = Url::parse(&format!("{scheme}://{ip}:{port}"))
        .map_err(|_| CliError::InternalError(format!("Couldn't parse url")))?;

    url.set_path(api_path);

    Ok(url)
}

pub async fn get_response<S>(
    client: &Client,
    url: Url,
    body: Option<S>,
    keypair: &Keypair,
    req_type: RequestType,
) -> Result<Response, CliError>
where
    S: Serialize,
{
    let timestamp = Utc::now().timestamp().to_string();
    let signature = keypair
        .sign(timestamp.as_bytes())
        .map_err(|_| CliError::InternalError(format!("Couldn't sign keypair")))?;

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
        .map_err(|_| CliError::InternalError(format!("Error with client request")))
}

pub fn load_config(path: &Utf8PathBuf) -> Result<ConfigFile, CliError> {
    if !ConfigFile::exists(&path) {
        println!("{}", path);
        return Err(CliError::InternalError(format!(
            "Config file does not exist"
        )));
    };

    let Ok(config) = ConfigFile::load(&path) else {
        return Err(CliError::InternalError(format!(
            "Failed to load config file"
        )));
    };

    Ok(config)
}

pub fn fetch_multiaddr(config: &ConfigFile) -> Result<&Multiaddr, CliError> {
    let Some(multiaddr) = config.network.server.listen.first() else {
        return Err(CliError::InternalError(format!("No address found")));
    };

    Ok(multiaddr)
}

#[allow(dead_code)]
pub enum RequestType {
    Get,
    Post,
    Delete,
}

#[derive(Debug)]
pub enum CliError {
    MethodCallError(String),
    InternalError(String),
}

pub trait ToResponseBody {
    fn to_res_body(self) -> ResponseBody;
}

impl<T: Serialize> ToResponseBody for Result<T, CliError> {
    fn to_res_body(self) -> ResponseBody {
        match self {
            Ok(r) => match to_value(r) {
                Ok(v) => ResponseBody::Result(v),
                Err(e) => ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::InternalError(e.to_string()),
                )),
            },
            Err(CliError::MethodCallError(err)) => match to_value(err) {
                Ok(v) => ResponseBody::Error(ResponseBodyError::HandlerError(v)),
                Err(e) => ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::InternalError(e.to_string()),
                )),
            },
            Err(CliError::InternalError(err)) => ResponseBody::Error(
                ResponseBodyError::ServerError(ServerResponseError::InternalError(err.to_string())),
            ),
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
    Error(ResponseBodyError),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ResponseBodyError {
    HandlerError(Value),
    ServerError(ServerResponseError),
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub enum ServerResponseError {
    ParseError(String),
    InternalError(String),
}
