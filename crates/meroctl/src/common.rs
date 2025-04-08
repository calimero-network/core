use std::fmt;
use std::str::FromStr;

use calimero_config::ConfigFile;
use calimero_primitives::alias::{Alias, ScopedAlias};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    AliasKind, CreateAliasRequest, CreateAliasResponse, CreateApplicationIdAlias,
    CreateContextIdAlias, CreateContextIdentityAlias, DeleteAliasResponse, LookupAliasResponse,
};
use camino::Utf8Path;
use chrono::Utc;
use eyre::{bail, eyre, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use reqwest::{Client, Url};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::cli::{ApiError, Environment};
use crate::output::Report;

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

pub(crate) async fn make_request<I, O>(
    environment: &Environment,
    client: &Client,
    url: Url,
    request: Option<I>,
    keypair: &Keypair,
    request_type: RequestType,
) -> EyreResult<()>
where
    I: Serialize,
    O: DeserializeOwned + Report + Serialize,
{
    let response = do_request::<I, O>(client, url, request, keypair, request_type).await?;
    environment.output.write(&response);
    Ok(())
}

pub(crate) trait UrlFragment: ScopedAlias + AliasKind {
    const KIND: &'static str;

    fn create(self) -> Self::Value;

    fn scoped(scope: &Self::Scope) -> Option<&str>;
}

impl UrlFragment for ContextId {
    const KIND: &'static str = "context";

    fn create(self) -> Self::Value {
        CreateContextIdAlias { context_id: self }
    }

    fn scoped(_: &Self::Scope) -> Option<&str> {
        None
    }
}

impl UrlFragment for PublicKey {
    const KIND: &'static str = "identity";

    fn create(self) -> Self::Value {
        CreateContextIdentityAlias { identity: self }
    }

    fn scoped(context: &Self::Scope) -> Option<&str> {
        Some(context.as_str())
    }
}

impl UrlFragment for ApplicationId {
    const KIND: &'static str = "application";

    fn create(self) -> Self::Value {
        CreateApplicationIdAlias {
            application_id: self,
        }
    }

    fn scoped(_: &Self::Scope) -> Option<&str> {
        None
    }
}

impl Report for CreateAliasResponse {
    fn report(&self) {
        println!("alias created");
    }
}

pub(crate) async fn create_alias<T>(
    multiaddr: &Multiaddr,
    keypair: &Keypair,
    alias: Alias<T>,
    scope: Option<T::Scope>,
    value: T,
) -> EyreResult<CreateAliasResponse>
where
    T: ScopedAlias + UrlFragment + Serialize,
    T::Value: Serialize,
{
    let prefix = "admin-api/dev/alias/create";

    let kind = T::KIND;

    let scope = scope
        .as_ref()
        .and_then(T::scoped)
        .map_or_else(Default::default, |scope| format!("/{}", scope));

    let body = CreateAliasRequest {
        alias,
        value: value.create(),
    };

    let response: CreateAliasResponse = do_request(
        &Client::new(),
        multiaddr_to_url(multiaddr, &format!("{prefix}/{kind}{scope}"))?,
        Some(body),
        keypair,
        RequestType::Post,
    )
    .await?;

    Ok(response)
}

impl Report for DeleteAliasResponse {
    fn report(&self) {
        println!("alias deleted");
    }
}

pub(crate) async fn delete_alias<T>(
    multiaddr: &Multiaddr,
    keypair: &Keypair,
    alias: Alias<T>,
    scope: Option<T::Scope>,
) -> EyreResult<DeleteAliasResponse>
where
    T: ScopedAlias + UrlFragment,
{
    let prefix = "admin-api/dev/alias/delete";

    let kind = T::KIND;

    let scope = scope
        .as_ref()
        .and_then(T::scoped)
        .map_or_else(Default::default, |scope| format!("{}/", scope));

    let response: DeleteAliasResponse = do_request(
        &Client::new(),
        multiaddr_to_url(multiaddr, &format!("{prefix}/{kind}/{scope}{alias}"))?,
        None::<()>,
        keypair,
        RequestType::Post,
    )
    .await?;

    Ok(response)
}

pub(crate) async fn lookup_alias<T>(
    multiaddr: &Multiaddr,
    keypair: &Keypair,
    alias: Alias<T>,
    scope: Option<T::Scope>,
) -> EyreResult<LookupAliasResponse<T>>
where
    T: ScopedAlias + UrlFragment + DeserializeOwned,
{
    let prefix = "admin-api/dev/alias/lookup";

    let kind = T::KIND;

    let scope = scope
        .as_ref()
        .and_then(T::scoped)
        .map_or_else(Default::default, |scope| format!("{}/", scope));

    let response = do_request(
        &Client::new(),
        multiaddr_to_url(multiaddr, &format!("{prefix}/{kind}/{scope}{alias}"))?,
        None::<()>,
        keypair,
        RequestType::Post,
    )
    .await?;

    Ok(response)
}

impl<T: fmt::Display> Report for LookupAliasResponse<T> {
    fn report(&self) {
        match &self.data.value {
            Some(value) => println!("aliased to {}", value),
            None => println!("alias not found"),
        }
    }
}

#[derive(Serialize)]
pub struct ResolveResponse<T> {
    alias: Alias<T>,
    value: Option<ResolveResponseValue<T>>,
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "data")]
enum ResolveResponseValue<T> {
    Lookup(LookupAliasResponse<T>),
    Parsed(T),
}

impl<T> ResolveResponse<T> {
    pub fn value(&self) -> Option<&T> {
        match self.value.as_ref()? {
            ResolveResponseValue::Lookup(value) => value.data.value.as_ref(),
            ResolveResponseValue::Parsed(value) => Some(value),
        }
    }
}

impl<T: fmt::Display> Report for ResolveResponse<T> {
    fn report(&self) {
        println!("{}", self.alias);
        match &self.value {
            Some(ResolveResponseValue::Lookup(value)) => value.report(),
            Some(ResolveResponseValue::Parsed(value)) => println!("parses to {}", value),
            None => println!("could not be reolved"),
        }
    }
}

pub(crate) async fn resolve_alias<T>(
    multiaddr: &Multiaddr,
    keypair: &Keypair,
    alias: Alias<T>,
    scope: Option<T::Scope>,
) -> EyreResult<ResolveResponse<T>>
where
    T: ScopedAlias + UrlFragment + FromStr + DeserializeOwned,
{
    let value = lookup_alias(multiaddr, keypair, alias, scope).await?;

    if value.data.value.is_some() {
        return Ok(ResolveResponse {
            alias,
            value: Some(ResolveResponseValue::Lookup(value)),
        });
    }

    let value = alias
        .as_str()
        .parse()
        .ok()
        .map(ResolveResponseValue::Parsed);

    Ok(ResolveResponse { alias, value })
}
