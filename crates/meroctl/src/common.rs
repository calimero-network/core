use calimero_config::ConfigFile;
use camino::Utf8Path;
use chrono::Utc;
use eyre::{bail, eyre, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use reqwest::{Client, Url};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::cli::ApiError;

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

pub async fn do_request<I, O>(
    client: &Client,
    url: Url,
    body: Option<I>,
    keypair: &Keypair,
    req_type: RequestType,
) -> EyreResult<O>
where
    I: Serialize,
    O: DeserializeOwned,
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

    let response = builder.send().await?;

    if !response.status().is_success() {
        bail!(ApiError {
            status_code: response.status().as_u16(),
            message: response.text().await?,
        });
    }

    let result = response.json::<O>().await?;

    Ok(result)
}
// pub async fn do_request<I, O>(
//     client: &Client,
//     url: Url,
//     body: Option<I>,
//     keypair: &Keypair,
//     req_type: RequestType,
// ) -> Result<O, ServerRequestError>
// where
//     I: Serialize,
//     O: DeserializeOwned,
// {
//     let timestamp = Utc::now().timestamp().to_string();
//     let signature = keypair
//         .sign(timestamp.as_bytes())
//         .map_err(|err| ServerRequestError::SigningError(err.to_string()))?;

//     let mut builder = match req_type {
//         RequestType::Get => client.get(url),
//         RequestType::Post => client.post(url).json(&body),
//         RequestType::Delete => client.delete(url),
//     };

//     builder = builder
//         .header("X-Signature", bs58::encode(signature).into_string())
//         .header("X-Timestamp", timestamp);

//     let response = builder
//         .send()
//         .await
//         .map_err(|err| ServerRequestError::ExecutionError(err.to_string()))?;

//     if !response.status().is_success() {
//         return Err(ServerRequestError::ApiError(ApiError {
//             status_code: response.status().as_u16(),
//             message: response
//                 .text()
//                 .await
//                 .map_err(|err| ServerRequestError::DeserializeError(err.to_string()))?,
//         }));
//     }

//     let result = response
//         .json::<O>()
//         .await
//         .map_err(|err| ServerRequestError::DeserializeError(err.to_string()))?;

//     return Ok(result);
// }

pub fn load_config(home: &Utf8Path, node_name: &str) -> EyreResult<ConfigFile> {
    let path = home.join(node_name);

    if !ConfigFile::exists(&path) {
        bail!("Config file does not exist")
    };

    let Ok(config) = ConfigFile::load(&path) else {
        bail!("Failed to load config file")
    };

    Ok(config)
}

pub fn fetch_multiaddr(config: &ConfigFile) -> EyreResult<&Multiaddr> {
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
