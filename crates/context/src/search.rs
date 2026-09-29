//! The execute path's side of full-text search (PoC).
//!
//! Three seams between the context manager and `calimero-search`:
//!
//! - [`changed_entity_ids`]: the entity ids a committed run touched, read from
//!   the `StorageDelta` the run already produced (a local write's artifact, a
//!   peer delta's payload). The execute path stages them as one `SearchDirty`
//!   row into the run's own transaction, so the row commits with the state.
//! - [`SearchHostAdapter`]: the runtime's `SearchHost`, handed only to
//!   read-only runs. The runtime passes the *running* context's id, never one
//!   the guest names.
//! - [`NodeExtractor`]: the indexer's way into the app — the app's search
//!   views, run through the ordinary execute path as this node's own member
//!   identity, read-only against current state.

use std::sync::Arc;

use async_trait::async_trait;
use calimero_context_client::client::ContextClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_primitives::search::{
    ExtractRequest, ExtractResponse, ScanRequest, ScanResponse, SearchIndexSchema, SearchRequest,
    EXTRACT_EXPORT, SCAN_EXPORT, SCHEMA_EXPORT,
};
use calimero_runtime::logic::SearchHost;
use calimero_search::{ContextKey, Extractor, SearchService};
use calimero_storage::delta::StorageDelta;
use eyre::{bail, eyre, Result as EyreResult};
use futures_util::StreamExt;

/// The entity ids `delta` (a borsh `StorageDelta`) touches, in first-seen
/// order, deduplicated. Empty for anything that does not decode.
#[must_use]
pub fn changed_entity_ids(delta: &[u8]) -> Vec<[u8; 32]> {
    let actions = match borsh::from_slice::<StorageDelta>(delta) {
        Ok(StorageDelta::Actions(actions) | StorageDelta::CausalActions { actions, .. }) => actions,
        _ => return Vec::new(),
    };
    let mut seen = std::collections::HashSet::with_capacity(actions.len());
    actions
        .iter()
        .map(|action| <[u8; 32]>::from(action.id()))
        .filter(|id| seen.insert(*id))
        .collect()
}

/// The search service as the runtime's `SearchHost`.
#[derive(Debug)]
pub struct SearchHostAdapter(pub Arc<SearchService>);

impl SearchHost for SearchHostAdapter {
    fn search(&self, context: [u8; 32], request: &[u8]) -> Result<Vec<u8>, String> {
        let request: SearchRequest = borsh::from_slice(request).map_err(|e| e.to_string())?;
        let response = self
            .0
            .search(&context, &request)
            .map_err(|e| e.to_string())?;
        borsh::to_vec(&response).map_err(|e| e.to_string())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> EyreResult<Vec<u8>> {
    if text.len() % 2 != 0 {
        bail!("odd-length hex");
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(Into::into))
        .collect()
}

/// The indexer's [`Extractor`]: the app's search views, run through the
/// context manager like any other call.
#[derive(Clone, Debug)]
pub struct NodeExtractor {
    context_client: ContextClient,
}

impl NodeExtractor {
    /// An extractor over `context_client`.
    #[must_use]
    pub fn new(context_client: ContextClient) -> Self {
        Self { context_client }
    }

    async fn executor(&self, context: &ContextId) -> EyreResult<PublicKey> {
        let mut members = Box::pin(self.context_client.get_context_members(context, Some(true)));
        while let Some(member) = members.next().await {
            let (key, _) = member?;
            return Ok(key);
        }
        bail!("this node holds no identity in context {context}")
    }

    /// Run `method` with `args` (a JSON object) and decode its hex-borsh
    /// result. `None` when the app does not have the method.
    async fn call<T: borsh::BorshDeserialize>(
        &self,
        context: ContextKey,
        method: &str,
        args: serde_json::Value,
    ) -> EyreResult<Option<T>> {
        let context = ContextId::from(context);
        let executor = self.executor(&context).await?;
        let response = match self
            .context_client
            .execute(
                &context,
                &executor,
                method.to_owned(),
                serde_json::to_vec(&args)?,
                None,
            )
            .await
        {
            Ok(response) => response,
            Err(calimero_context_client::messages::ExecuteError::Uninitialized) => return Ok(None),
            Err(err) => return Err(eyre!("{method}: {err}")),
        };
        let bytes = match response.returns {
            Ok(Some(bytes)) => bytes,
            Ok(None) => bail!("{method} returned nothing"),
            // An app without search has no such method.
            Err(err) if err.to_string().contains("MethodNotFound") => return Ok(None),
            Err(err) => return Err(err.wrap_err(method.to_owned())),
        };
        let text: String = serde_json::from_slice(&bytes)?;
        Ok(Some(borsh::from_slice(&unhex(&text)?)?))
    }
}

#[async_trait]
impl Extractor for NodeExtractor {
    async fn schema(&self, context: ContextKey) -> EyreResult<Option<Vec<SearchIndexSchema>>> {
        self.call(context, SCHEMA_EXPORT, serde_json::json!({}))
            .await
    }

    async fn extract(
        &self,
        context: ContextKey,
        index: &str,
        ids: Vec<[u8; 32]>,
    ) -> EyreResult<ExtractResponse> {
        let request = hex(&borsh::to_vec(&ExtractRequest {
            index: index.to_owned(),
            ids,
        })?);
        self.call(
            context,
            EXTRACT_EXPORT,
            serde_json::json!({ "request": request }),
        )
        .await?
        .ok_or_else(|| eyre!("the app lost its {EXTRACT_EXPORT} method"))
    }

    async fn scan(
        &self,
        context: ContextKey,
        index: &str,
        offset: u32,
        limit: u32,
    ) -> EyreResult<ScanResponse> {
        let request = hex(&borsh::to_vec(&ScanRequest {
            index: index.to_owned(),
            offset,
            limit,
        })?);
        self.call(
            context,
            SCAN_EXPORT,
            serde_json::json!({ "request": request }),
        )
        .await?
        .ok_or_else(|| eyre!("the app lost its {SCAN_EXPORT} method"))
    }
}

#[cfg(test)]
mod tests {
    use calimero_storage::action::Action;
    use calimero_storage::address::Id;
    use calimero_storage::entities::Metadata;

    use super::*;

    #[test]
    fn ids_come_from_every_action_once() {
        let add = |id: u8| Action::Add {
            id: Id::new([id; 32]),
            data: vec![1, 2, 3],
            ancestors: vec![],
            metadata: Metadata::default(),
        };
        let delta = borsh::to_vec(&StorageDelta::Actions(vec![add(1), add(2), add(1)])).unwrap();
        assert_eq!(changed_entity_ids(&delta), vec![[1; 32], [2; 32]]);
        assert!(changed_entity_ids(b"not a delta").is_empty());
    }
}
