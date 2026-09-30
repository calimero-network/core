//! The execute path's side of full-text search.
//!
//! Three seams between the context manager and `calimero-search`:
//!
//! - [`changed_entity_ids`]: the entity ids a committed run touched, read from
//!   the `StorageDelta` the run already produced (a local write's artifact, a
//!   peer delta's payload). The execute path stages them, with the state root
//!   before and after, as one `SearchDirty` row in the run's own transaction,
//!   so the row commits with the state.
//! - [`SearchHostAdapter`]: the runtime's `SearchHost`, handed only to
//!   read-only runs. The runtime passes the *running* context's id, never one
//!   the guest names.
//! - [`NodeContextSource`]: the indexer's way into the app and its state — the
//!   app's search exports, run through the ordinary execute path as this
//!   node's own member identity, read-only against current state; the state
//!   root; and the context lock.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use calimero_context_client::client::ContextClient;
use calimero_context_client::messages::ExecuteError;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_primitives::search::{
    ExtractRequest, ExtractResponse, ScanRequest, ScanResponse, SearchIndexSchema, SearchRequest,
    EXTRACT_EXPORT, SCAN_EXPORT, SCHEMA_EXPORT,
};
use calimero_runtime::errors::{FunctionCallError, MethodResolutionError};
use calimero_runtime::logic::{SearchHost, SearchOutput};
use calimero_search::{ContextKey, ContextSource, SearchService};
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
    let mut seen = HashSet::with_capacity(actions.len());
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
    fn search(&self, context: [u8; 32], request: &[u8]) -> Result<SearchOutput, String> {
        let request: SearchRequest = borsh::from_slice(request).map_err(|e| e.to_string())?;
        let response = self
            .0
            .search(&context, &request)
            .map_err(|e| e.to_string())?;
        Ok(SearchOutput {
            matched: response.total,
            hits: response.hits.len() as u64,
            response: borsh::to_vec(&response).map_err(|e| e.to_string())?,
        })
    }
}

/// The indexer's [`ContextSource`]: the app's search exports, run through the
/// context manager like any other call, and the context's state.
#[derive(Clone, Debug)]
pub struct NodeContextSource {
    context_client: ContextClient,
}

impl NodeContextSource {
    /// A source over `context_client`.
    #[must_use]
    pub fn new(context_client: ContextClient) -> Self {
        Self { context_client }
    }

    async fn executor(&self, context: &ContextId) -> EyreResult<PublicKey> {
        let mut members = Box::pin(self.context_client.get_context_members(context, Some(true)));
        match members.next().await {
            Some(member) => Ok(member?.0),
            None => bail!("this node holds no identity in context {context}"),
        }
    }

    /// Run export `method` on `input` (borsh) and decode its borsh result.
    /// `None` when the app does not have the method, or the context is not
    /// initialized yet or no longer exists.
    async fn call<T: borsh::BorshDeserialize>(
        &self,
        context: ContextKey,
        method: &str,
        input: Vec<u8>,
    ) -> EyreResult<Option<T>> {
        let context = ContextId::from(context);
        let executor = self.executor(&context).await?;
        let response = match self
            .context_client
            .execute(&context, &executor, method.to_owned(), input, None)
            .await
        {
            Ok(response) => response,
            // Not initialized yet, or deleted since it was queued: nothing to
            // index, and a pass that finds no schema drops what was built.
            Err(ExecuteError::Uninitialized | ExecuteError::ContextNotFound) => return Ok(None),
            Err(err) => return Err(eyre!("{method}: {err}")),
        };
        let bytes = match response.returns {
            Ok(Some(bytes)) => bytes,
            Ok(None) => bail!("{method} returned nothing"),
            Err(err)
                if matches!(
                    err.downcast_ref::<FunctionCallError>(),
                    Some(FunctionCallError::MethodResolutionError(
                        MethodResolutionError::MethodNotFound { .. }
                    ))
                ) =>
            {
                return Ok(None)
            }
            Err(err) => return Err(err.wrap_err(method.to_owned())),
        };
        Ok(Some(borsh::from_slice(&bytes)?))
    }
}

#[async_trait]
impl ContextSource for NodeContextSource {
    async fn schema(&self, context: ContextKey) -> EyreResult<Option<Vec<SearchIndexSchema>>> {
        self.call(context, SCHEMA_EXPORT, Vec::new()).await
    }

    async fn extract(
        &self,
        context: ContextKey,
        index: &str,
        ids: Vec<[u8; 32]>,
    ) -> EyreResult<ExtractResponse> {
        let request = borsh::to_vec(&ExtractRequest {
            index: index.to_owned(),
            ids,
        })?;
        self.call(context, EXTRACT_EXPORT, request)
            .await?
            .ok_or_else(|| eyre!("the app lost its {EXTRACT_EXPORT} export"))
    }

    async fn scan(
        &self,
        context: ContextKey,
        index: &str,
        from: [u8; 32],
        limit: u32,
    ) -> EyreResult<ScanResponse> {
        let request = borsh::to_vec(&ScanRequest {
            index: index.to_owned(),
            from,
            limit,
        })?;
        self.call(context, SCAN_EXPORT, request)
            .await?
            .ok_or_else(|| eyre!("the app lost its {SCAN_EXPORT} export"))
    }

    fn state_root(&self, context: ContextKey) -> EyreResult<[u8; 32]> {
        self.context_client
            .compute_root_hash(&ContextId::from(context))
    }

    async fn lock(&self, context: ContextKey) -> EyreResult<Box<dyn Send>> {
        Ok(Box::new(
            self.context_client
                .acquire_lock(&ContextId::from(context))
                .await,
        ))
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
