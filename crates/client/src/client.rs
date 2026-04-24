//! API client for Calimero services
//!
//! This module provides the core client functionality for making
//! authenticated API requests to Calimero services.

// Standard library
use std::str::FromStr;

// External crates
use calimero_primitives::alias::{Alias, ScopedAlias};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    AliasKind, CreateAliasRequest, CreateAliasResponse, CreateApplicationIdAlias,
    CreateContextIdAlias, CreateContextIdentityAlias, DeleteAliasResponse, ListAliasesResponse,
    LookupAliasResponse,
};
use eyre::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

// Local crate
use crate::connection::ConnectionInfo;
use crate::traits::{ClientAuthenticator, ClientStorage};

mod applications;
mod blobs;
mod contexts;
mod group;
mod jsonrpc;
mod namespace;
mod packages;
mod system;

pub trait UrlFragment: ScopedAlias + AliasKind {
    const KIND: &'static str;

    fn create(self) -> Self::Value;

    fn scoped(scope: Option<&Self::Scope>) -> Option<String>;
}

impl UrlFragment for ContextId {
    const KIND: &'static str = "context";

    fn create(self) -> Self::Value {
        CreateContextIdAlias { context_id: self }
    }

    fn scoped(_: Option<&Self::Scope>) -> Option<String> {
        None
    }
}

impl UrlFragment for PublicKey {
    const KIND: &'static str = "identity";

    fn create(self) -> Self::Value {
        CreateContextIdentityAlias { identity: self }
    }

    fn scoped(context: Option<&Self::Scope>) -> Option<String> {
        context.map(ContextId::to_string)
    }
}

impl UrlFragment for ApplicationId {
    const KIND: &'static str = "application";

    fn create(self) -> Self::Value {
        CreateApplicationIdAlias {
            application_id: self,
        }
    }

    fn scoped(_: Option<&Self::Scope>) -> Option<String> {
        None
    }
}

#[derive(Debug, Serialize)]
pub struct ResolveResponse<T> {
    alias: Alias<T>,
    value: Option<ResolveResponseValue<T>>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "data")]
pub enum ResolveResponseValue<T> {
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

    pub fn alias(&self) -> &Alias<T> {
        &self.alias
    }

    pub fn value_enum(&self) -> Option<&ResolveResponseValue<T>> {
        self.value.as_ref()
    }
}

/// Generic API client that can work with any authenticator and storage implementation
#[derive(Clone, Debug)]
pub struct Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    connection: ConnectionInfo<A, S>,
}

impl<A, S> Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub fn new(connection: ConnectionInfo<A, S>) -> Result<Self> {
        Ok(Self { connection })
    }

    pub fn api_url(&self) -> &Url {
        &self.connection.api_url
    }

    /// Create context identity alias (legacy method for backward compatibility)
    pub async fn create_context_identity_alias(
        &self,
        context_id: &ContextId,
        request: CreateAliasRequest<PublicKey>,
    ) -> Result<CreateAliasResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/alias/create/identity/{}", context_id),
                request,
            )
            .await?;
        Ok(response)
    }

    /// Create alias generic (legacy method for backward compatibility)
    pub async fn create_alias_generic<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
        value: T,
    ) -> Result<CreateAliasResponse>
    where
        T: UrlFragment + Serialize,
        T::Value: Serialize,
    {
        self.create_alias(alias, value, scope).await
    }

    pub async fn create_alias<T>(
        &self,
        alias: Alias<T>,
        value: T,
        scope: Option<T::Scope>,
    ) -> Result<CreateAliasResponse>
    where
        T: UrlFragment + Serialize,
        T::Value: Serialize,
    {
        let prefix = "admin-api/alias/create";
        let kind = T::KIND;
        let scope_path = T::scoped(scope.as_ref())
            .map(|scope| format!("/{}", scope))
            .unwrap_or_default();

        let body = CreateAliasRequest {
            alias,
            value: value.create(),
        };

        let response = self
            .connection
            .post(&format!("{prefix}/{kind}{scope_path}"), body)
            .await?;
        Ok(response)
    }

    pub async fn delete_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
    ) -> Result<DeleteAliasResponse>
    where
        T: UrlFragment,
    {
        let prefix = "admin-api/alias/delete";
        let kind = T::KIND;
        let scope_path = T::scoped(scope.as_ref())
            .map(|scope| format!("/{}", scope))
            .unwrap_or_default();

        let response = self
            .connection
            .post_no_body(&format!("{prefix}/{kind}{scope_path}/{alias}"))
            .await?;
        Ok(response)
    }

    pub async fn list_aliases<T>(&self, scope: Option<T::Scope>) -> Result<ListAliasesResponse<T>>
    where
        T: Ord + UrlFragment + DeserializeOwned,
    {
        let prefix = "admin-api/alias/list";
        let kind = T::KIND;
        let scope_path = T::scoped(scope.as_ref())
            .map(|scope| format!("/{}", scope))
            .unwrap_or_default();

        let response = self
            .connection
            .get(&format!("{prefix}/{kind}{scope_path}"))
            .await?;
        Ok(response)
    }

    pub async fn lookup_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
    ) -> Result<LookupAliasResponse<T>>
    where
        T: UrlFragment + DeserializeOwned,
    {
        let prefix = "admin-api/alias/lookup";
        let kind = T::KIND;
        let scope_path = T::scoped(scope.as_ref())
            .map(|scope| format!("/{}", scope))
            .unwrap_or_default();

        let response = self
            .connection
            .post_no_body(&format!("{prefix}/{kind}{scope_path}/{alias}"))
            .await?;
        Ok(response)
    }

    pub async fn resolve_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
    ) -> Result<ResolveResponse<T>>
    where
        T: UrlFragment + FromStr + DeserializeOwned,
    {
        let value = self.lookup_alias(alias, scope).await?;

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
}
