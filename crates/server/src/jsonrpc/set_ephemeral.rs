//! JSON-RPC `set_ephemeral` handler.
//!
//! Writes the caller's local ephemeral-presence slice for a context. The
//! author identity is resolved server-side (the node's owned key for the
//! context, obtained via `get_context_members(owned=true)`) — callers never
//! specify it, mirroring the `execute` convention.
//!
//! The handler calls [`NodeClient::set_local_ephemeral`] which routes through
//! the `NodeManager` actor so seq-counter management and the async gossip-
//! publish stay on the actor's Arbiter.

use std::pin::pin;
use std::sync::Arc;

use calimero_primitives::events::EPHEMERAL_MAX_BYTES;
use calimero_server_primitives::jsonrpc::{
    SetEphemeralError, SetEphemeralRequest, SetEphemeralResponse,
};
use futures_util::StreamExt;
use tracing::debug;

use super::{Request, RpcError, ServiceState};
use crate::auth::{AuthenticatedKey, AuthenticatedNodeOwner};

impl Request for SetEphemeralRequest {
    type Response = SetEphemeralResponse;
    type Error = SetEphemeralError;

    async fn handle(
        self,
        state: Arc<ServiceState>,
        _auth_key: Option<AuthenticatedKey>,
        _auth_node_owner: Option<AuthenticatedNodeOwner>,
    ) -> Result<Self::Response, RpcError<Self::Error>> {
        let context_id = self.context_id;

        // Reject oversize slices up front with the typed `SliceTooLarge` error
        // so the client learns the exact cap, rather than a stringified opaque
        // node error. `EPHEMERAL_MAX_BYTES` is the single source of truth in
        // `calimero-primitives`, the same const the node's outbound path
        // enforces — they cannot drift. The node still re-checks (defense in
        // depth); this handler pre-check only exists to produce the typed wire
        // error before the round-trip through the actor.
        if self.state.len() > EPHEMERAL_MAX_BYTES {
            return Err(RpcError::MethodCallError(
                SetEphemeralError::SliceTooLarge {
                    size: self.state.len(),
                    max: EPHEMERAL_MAX_BYTES,
                },
            ));
        }

        // Auto-resolve the node's owned identity for this context.
        // Each node has exactly one owned identity per context (the namespace
        // identity). Mirroring the `execute` path: the first member returned
        // by `get_context_members(owned=true)` is that identity.
        let author = {
            let members = state
                .ctx_client
                .get_context_members(&context_id, Some(true));
            let mut members = pin!(members);
            match members.next().await {
                Some(Ok((public_key, _))) => public_key,
                Some(Err(_)) | None => {
                    debug!(%context_id, "set_ephemeral: no owned identity for context");
                    return Err(RpcError::MethodCallError(
                        SetEphemeralError::NoOwnedIdentity,
                    ));
                }
            }
        };

        // Delegate to the node actor, which re-enforces EPHEMERAL_MAX_BYTES and
        // drives the seq counter + async publish. Any error surfacing here is a
        // key-loading/crypto/transport failure (the size case is caught above),
        // so it maps to the catch-all `InternalError`.
        state
            .node_client
            .set_local_ephemeral(context_id, author, self.state)
            .await
            .map_err(|err| {
                RpcError::MethodCallError(SetEphemeralError::InternalError(err.to_string()))
            })?;

        Ok(SetEphemeralResponse::default())
    }
}

#[cfg(test)]
mod tests {
    //! Wire-shape tests for the `set_ephemeral` request/response types.
    //!
    //! These verify that the serde attributes (`camelCase`, tag/content layout)
    //! produce the expected JSON, catching regressions before they reach clients.

    use calimero_primitives::context::ContextId;
    use calimero_server_primitives::jsonrpc::{
        Request, RequestId, RequestPayload, SetEphemeralRequest, SetEphemeralResponse, Version,
    };
    use serde_json::{json, Value};

    #[test]
    fn set_ephemeral_request_method_tag_is_snake_case() {
        let ctx_id = ContextId::from([0xAB; 32]);
        let request = Request::new(
            Version::TwoPointZero,
            RequestId::Null,
            RequestPayload::SetEphemeral(SetEphemeralRequest::new(ctx_id, vec![1, 2, 3])),
        );
        let json_val: Value = serde_json::to_value(&request).expect("serialize");
        assert_eq!(
            json_val.get("method").and_then(Value::as_str),
            Some("set_ephemeral"),
            "method tag must be snake_case"
        );
    }

    #[test]
    fn set_ephemeral_request_context_id_field_is_camel_case() {
        let ctx_id = ContextId::from([0x01; 32]);
        let req = SetEphemeralRequest::new(ctx_id, vec![]);
        let json_val: Value = serde_json::to_value(&req).expect("serialize");
        assert!(
            json_val.get("contextId").is_some(),
            "contextId must be present (camelCase)"
        );
        assert!(
            json_val.get("context_id").is_none(),
            "context_id must NOT be present (snake_case)"
        );
    }

    #[test]
    fn set_ephemeral_request_round_trips() {
        let ctx_id = ContextId::from([0x02; 32]);
        let state = vec![104u8, 101, 108, 108, 111]; // b"hello"
        let original = SetEphemeralRequest::new(ctx_id, state.clone());
        let json_str = serde_json::to_string(&original).expect("serialize");
        let decoded: SetEphemeralRequest = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(decoded.context_id, ctx_id);
        assert_eq!(decoded.state, state);
    }

    #[test]
    fn set_ephemeral_response_is_empty_object() {
        let resp = SetEphemeralResponse::default();
        let json_val: Value = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(json_val, json!({}), "SetEphemeralResponse must be {{}}");
    }

    #[test]
    fn request_payload_deserializes_set_ephemeral() {
        let ctx_id = ContextId::from([0x03; 32]);
        // The RequestPayload is tagged with `method` + `params`.
        let json_str = serde_json::to_string(&serde_json::json!({
            "method": "set_ephemeral",
            "params": {
                "contextId": serde_json::to_value(ctx_id).unwrap(),
                "state": [1u8, 2, 3]
            }
        }))
        .unwrap();
        let payload: RequestPayload =
            serde_json::from_str(&json_str).expect("must deserialize as SetEphemeral");
        assert!(
            matches!(payload, RequestPayload::SetEphemeral(_)),
            "wrong variant: {payload:?}"
        );
    }

    /// The typed `SliceTooLarge` error serializes with the `size`/`max` fields
    /// under the tagged `{type, data}` envelope, so a JS client can read the
    /// exact cap it exceeded rather than parsing an opaque string. This is the
    /// wire proof for I1 (the variant is real and reachable, not dead API).
    #[test]
    fn slice_too_large_error_wire_shape() {
        use calimero_server_primitives::jsonrpc::SetEphemeralError;

        let err = SetEphemeralError::SliceTooLarge {
            size: 20_000,
            max: 16_384,
        };
        let json_val: Value = serde_json::to_value(&err).expect("serialize");
        assert_eq!(
            json_val,
            json!({ "type": "SliceTooLarge", "data": { "size": 20_000, "max": 16_384 } }),
            "SliceTooLarge must carry size + max under the tagged envelope"
        );
    }
}

/// Handler-path tests: drive `SetEphemeralRequest::handle` against a real
/// (in-memory-backed) `ServiceState` to prove the oversize guard returns the
/// typed `SliceTooLarge` BEFORE any node round-trip (the size check is the
/// first statement in the handler, so the stub clients are never reached).
#[cfg(test)]
mod handler_tests {
    use std::sync::Arc;

    use calimero_blobstore::config::BlobStoreConfig;
    use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
    use calimero_context_client::client::ContextClient;
    use calimero_network_primitives::client::NetworkClient;
    use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
    use calimero_primitives::context::ContextId;
    use calimero_primitives::events::{NodeEvent, EPHEMERAL_MAX_BYTES};
    use calimero_server_primitives::jsonrpc::{SetEphemeralError, SetEphemeralRequest};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use tempfile::TempDir;
    use tokio::sync::{broadcast, mpsc};

    use super::super::{Request, RpcError, ServiceState};

    async fn minimal_state() -> (Arc<ServiceState>, TempDir) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let blob_dir = TempDir::new().expect("tempdir");
        let blob_store = BlobStore::new(
            store.clone(),
            FileSystem::new(&BlobStoreConfig::new(
                blob_dir.path().to_path_buf().try_into().expect("utf8 path"),
            ))
            .await
            .expect("blob fs"),
        );
        let blob_manager = BlobManager::new(blob_store);

        let (event_sender, _) = broadcast::channel::<NodeEvent>(16);
        let (ctx_sync_tx, _r0) = mpsc::channel(8);
        let (ns_sync_tx, _r1) = mpsc::channel(8);
        let (ns_join_tx, _r2) = mpsc::channel(8);
        let (open_subgroup_join_tx, _r3) = mpsc::channel(8);
        let sync_client =
            SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

        let node_client = NodeClient::new(
            store.clone(),
            blob_manager,
            NetworkClient::new(LazyRecipient::new()),
            LazyRecipient::new(),
            event_sender,
            sync_client,
            String::new(),
            None,
        );
        let ctx_client = ContextClient::new(store, node_client.clone(), LazyRecipient::new());

        let state = Arc::new(ServiceState {
            ctx_client,
            node_client,
            auth_enabled: false,
        });
        (state, blob_dir)
    }

    #[tokio::test]
    async fn oversize_slice_returns_typed_slice_too_large() {
        let (state, _blob_dir) = minimal_state().await;
        let ctx_id = ContextId::from([0x05; 32]);
        let oversized = vec![0xAB_u8; EPHEMERAL_MAX_BYTES + 1];
        let req = SetEphemeralRequest::new(ctx_id, oversized);

        let result = req.handle(state, None, None).await;

        match result {
            Err(RpcError::MethodCallError(SetEphemeralError::SliceTooLarge { size, max })) => {
                assert_eq!(
                    size,
                    EPHEMERAL_MAX_BYTES + 1,
                    "size must be the actual length"
                );
                assert_eq!(max, EPHEMERAL_MAX_BYTES, "max must be the shared cap");
            }
            other => panic!("expected typed SliceTooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn exact_cap_slice_passes_the_size_guard() {
        // A slice exactly at the cap must NOT trip the size guard; it proceeds
        // to author resolution, which fails (no context) with NoOwnedIdentity —
        // proving the boundary is `> max`, not `>= max`.
        let (state, _blob_dir) = minimal_state().await;
        let ctx_id = ContextId::from([0x06; 32]);
        let at_cap = vec![0xCD_u8; EPHEMERAL_MAX_BYTES];
        let req = SetEphemeralRequest::new(ctx_id, at_cap);

        let result = req.handle(state, None, None).await;

        assert!(
            matches!(
                result,
                Err(RpcError::MethodCallError(SetEphemeralError::NoOwnedIdentity))
            ),
            "at-cap slice must clear the size guard and fail later at identity resolution, got {result:?}"
        );
    }
}
