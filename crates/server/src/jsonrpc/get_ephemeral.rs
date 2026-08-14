//! JSON-RPC `get_ephemeral` handler.
//!
//! Returns a snapshot of live ephemeral-presence entries for a context from
//! the node's in-memory [`AwarenessStore`]. The snapshot is the client's
//! initial seed; live deltas arrive over the SSE/WS event stream.

use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{
    EphemeralEntryValue, GetEphemeralError, GetEphemeralRequest, GetEphemeralResponse,
};
use tracing::error;

use super::{Request, RpcError, ServiceState};
use crate::auth::{AuthenticatedKey, AuthenticatedNodeOwner};

impl Request for GetEphemeralRequest {
    type Response = GetEphemeralResponse;
    type Error = GetEphemeralError;

    async fn handle(
        self,
        state: Arc<ServiceState>,
        auth_key: Option<AuthenticatedKey>,
        auth_node_owner: Option<AuthenticatedNodeOwner>,
    ) -> Result<Self::Response, RpcError<Self::Error>> {
        // Authorization first: the snapshot is DECRYPTED presence (cursors,
        // typing state) for the requested context. Without this gate any
        // caller who can reach the endpoint could read live presence for a
        // context it is not a member of, by guessing a ContextId. The three
        // auth paths (key / node-owner / no-auth mode) resolve exactly as they
        // do for `execute` — see `super::caller_identity`.
        let caller = super::caller_identity(
            &state,
            auth_key.as_ref(),
            auth_node_owner.as_ref(),
            "get_ephemeral",
        )?;

        if !crate::execute::caller_authorized_for_context(
            &state.ctx_client,
            &self.context_id,
            &caller,
        )
        .map_err(|err| {
            error!(%err, context_id=%self.context_id, "Membership lookup failed during get_ephemeral");
            RpcError::MethodCallError(GetEphemeralError::InternalError(
                "internal error during membership verification".to_owned(),
            ))
        })? {
            return Err(RpcError::MethodCallError(GetEphemeralError::Unauthorized));
        }

        let entries = state
            .node_client
            .ephemeral_snapshot(self.context_id)
            .await
            .map_err(|err| {
                RpcError::MethodCallError(GetEphemeralError::InternalError(err.to_string()))
            })?;

        // Author-keyed: the store is already a per-author map and the event
        // stream delivers per-author deltas, so the snapshot keeps the same
        // shape rather than making every caller rebuild it. `author` is unique
        // within a context, so no entry can be lost to a key collision.
        Ok(GetEphemeralResponse::new(
            entries
                .into_iter()
                .map(|(author, state, age_ms)| {
                    (author.to_string(), EphemeralEntryValue::new(state, age_ms))
                })
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    //! Wire-shape tests for the `get_ephemeral` request/response types.

    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::PublicKey;
    use calimero_server_primitives::jsonrpc::{
        EphemeralEntryValue, GetEphemeralRequest, GetEphemeralResponse, Request, RequestId,
        RequestPayload, Version,
    };
    use serde_json::{json, Value};

    #[test]
    fn get_ephemeral_request_method_tag_is_snake_case() {
        let ctx_id = ContextId::from([0xCD; 32]);
        let request = Request::new(
            Version::TwoPointZero,
            RequestId::Null,
            RequestPayload::GetEphemeral(GetEphemeralRequest::new(ctx_id)),
        );
        let json_val: Value = serde_json::to_value(&request).expect("serialize");
        assert_eq!(
            json_val.get("method").and_then(Value::as_str),
            Some("get_ephemeral"),
            "method tag must be snake_case"
        );
    }

    #[test]
    fn get_ephemeral_response_round_trips() {
        let author = PublicKey::from([0xAA; 32]);
        let state_bytes = vec![9u8, 8, 7];
        let mut entries = std::collections::BTreeMap::new();
        let _ignored = entries.insert(
            author.to_string(),
            EphemeralEntryValue::new(state_bytes.clone(), 1_250),
        );
        let resp = GetEphemeralResponse::new(entries);
        let json_str = serde_json::to_string(&resp).expect("serialize");
        let decoded: GetEphemeralResponse = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(decoded.entries.len(), 1);
        let value = decoded
            .entries
            .get(&author.to_string())
            .expect("entry keyed by the author's string form");
        assert_eq!(value.state, state_bytes);
        assert_eq!(value.age_ms, 1_250);
    }

    #[test]
    fn get_ephemeral_response_entries_is_an_object_keyed_by_author() {
        // The snapshot is author-keyed, matching the per-author deltas on the
        // event stream. A regression to a list would break every client that
        // indexes by author.
        let author = PublicKey::from([0xAA; 32]);
        let mut entries = std::collections::BTreeMap::new();
        let _ignored = entries.insert(author.to_string(), EphemeralEntryValue::new(vec![1], 42));
        let json_val: Value =
            serde_json::to_value(&GetEphemeralResponse::new(entries)).expect("serialize");
        let obj = json_val
            .get("entries")
            .and_then(Value::as_object)
            .expect("entries must be a JSON object, not an array");
        let value = obj
            .get(&author.to_string())
            .expect("keyed by the author's base58 string");
        assert_eq!(value.get("state"), Some(&json!([1])));
        assert_eq!(
            value.get("ageMs"),
            Some(&json!(42)),
            "age must be camelCase on the wire"
        );
        assert!(
            value.get("author").is_none(),
            "author is the map key, not a field"
        );
    }

    #[test]
    fn get_ephemeral_response_empty_is_an_empty_object() {
        let resp = GetEphemeralResponse::new(std::collections::BTreeMap::new());
        let json_val: Value = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(json_val, json!({ "entries": {} }));
    }

    #[test]
    fn request_payload_deserializes_get_ephemeral() {
        let ctx_id = ContextId::from([0x04; 32]);
        let json_str = serde_json::to_string(&serde_json::json!({
            "method": "get_ephemeral",
            "params": {
                "contextId": serde_json::to_value(ctx_id).unwrap()
            }
        }))
        .unwrap();
        let payload: RequestPayload =
            serde_json::from_str(&json_str).expect("must deserialize as GetEphemeral");
        assert!(
            matches!(payload, RequestPayload::GetEphemeral(_)),
            "wrong variant: {payload:?}"
        );
    }
}

/// Handler-path tests: drive `GetEphemeralRequest::handle` against a real
/// (in-memory-backed) `ServiceState` to prove the authorization gate runs
/// before the snapshot is read, and that a member still gets the snapshot.
#[cfg(test)]
mod handler_tests {
    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::PublicKey;
    use calimero_server_primitives::jsonrpc::{GetEphemeralError, GetEphemeralRequest};
    use calimero_utils_actix::LazyRecipient;

    use super::super::test_support::{seed_context_member, state_with, stub_node_manager};
    use super::super::{Request, RpcError};
    use crate::auth::{AuthenticatedKey, AuthenticatedNodeOwner};

    /// A token-authenticated key that is NOT a member of the target context
    /// must be refused. The snapshot is DECRYPTED presence, so without this
    /// gate any caller could read live cursors/typing state for a context it
    /// has no membership in, just by naming the ContextId.
    #[actix::test]
    async fn non_member_key_is_refused() {
        let author = PublicKey::from([0xAA; 32]);
        let node_manager = stub_node_manager(vec![(author, vec![1, 2, 3], 10)]);
        let t = state_with(true, node_manager).await;
        let ctx_id = ContextId::from([0x11; 32]);
        let stranger = PublicKey::from([0xEE; 32]);

        let result = GetEphemeralRequest::new(ctx_id)
            .handle(t.state.clone(), Some(AuthenticatedKey(stranger)), None)
            .await;

        assert!(
            matches!(
                result,
                Err(RpcError::MethodCallError(GetEphemeralError::Unauthorized))
            ),
            "a non-member key must be refused, got {result:?}"
        );
    }

    /// A member key clears the gate and receives the snapshot — the refusal
    /// above is about membership, not a blanket failure of the endpoint.
    #[actix::test]
    async fn member_key_receives_the_snapshot() {
        let author = PublicKey::from([0xAA; 32]);
        let node_manager = stub_node_manager(vec![(author, vec![1, 2, 3], 10)]);
        let t = state_with(true, node_manager).await;
        let ctx_id = ContextId::from([0x12; 32]);
        let member = PublicKey::from([0xBB; 32]);
        seed_context_member(&t.store, ctx_id, member);

        let response = GetEphemeralRequest::new(ctx_id)
            .handle(t.state.clone(), Some(AuthenticatedKey(member)), None)
            .await
            .expect("a member must be served the snapshot");

        let entry = response
            .entries
            .get(&author.to_string())
            .expect("snapshot entry keyed by author");
        assert_eq!(entry.state, vec![1, 2, 3]);
        assert_eq!(entry.age_ms, 10);
    }

    /// Node-owner auth skips the membership check, exactly as `execute` does.
    #[actix::test]
    async fn node_owner_skips_the_membership_check() {
        let author = PublicKey::from([0xAA; 32]);
        let node_manager = stub_node_manager(vec![(author, vec![7], 1)]);
        let t = state_with(true, node_manager).await;
        let ctx_id = ContextId::from([0x13; 32]);

        let response = GetEphemeralRequest::new(ctx_id)
            .handle(t.state.clone(), None, Some(AuthenticatedNodeOwner))
            .await
            .expect("node owner must be served the snapshot");

        assert_eq!(response.entries.len(), 1);
    }

    /// No-auth mode (the intentional no-auth deployment) still serves the
    /// snapshot — the carve-out must not be tightened by this gate.
    #[actix::test]
    async fn no_auth_mode_still_serves_the_snapshot() {
        let author = PublicKey::from([0xAA; 32]);
        let node_manager = stub_node_manager(vec![(author, vec![7], 1)]);
        let t = state_with(false, node_manager).await;
        let ctx_id = ContextId::from([0x14; 32]);

        let response = GetEphemeralRequest::new(ctx_id)
            .handle(t.state.clone(), None, None)
            .await
            .expect("no-auth mode must behave as it did before the gate");

        assert_eq!(response.entries.len(), 1);
    }

    /// Auth enabled but no extension injected == a misconfigured guard. Reject
    /// rather than silently granting node-owner privileges.
    #[actix::test]
    async fn auth_enabled_without_extensions_is_rejected() {
        let t = state_with(true, LazyRecipient::new()).await;
        let ctx_id = ContextId::from([0x15; 32]);

        let result = GetEphemeralRequest::new(ctx_id)
            .handle(t.state.clone(), None, None)
            .await;

        assert!(
            matches!(result, Err(RpcError::InternalError(_))),
            "a missing auth extension under auth_enabled must be rejected, got {result:?}"
        );
    }
}
