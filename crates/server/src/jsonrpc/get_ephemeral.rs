//! JSON-RPC `get_ephemeral` handler.
//!
//! Returns a snapshot of live ephemeral-presence entries for a context from
//! the node's in-memory [`AwarenessStore`]. The snapshot is the client's
//! initial seed; live deltas arrive over the SSE/WS event stream.

use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{
    EphemeralEntryValue, GetEphemeralError, GetEphemeralRequest, GetEphemeralResponse,
};

use super::{Request, RpcError, ServiceState};
use crate::auth::{AuthenticatedKey, AuthenticatedNodeOwner};

impl Request for GetEphemeralRequest {
    type Response = GetEphemeralResponse;
    type Error = GetEphemeralError;

    async fn handle(
        self,
        state: Arc<ServiceState>,
        _auth_key: Option<AuthenticatedKey>,
        _auth_node_owner: Option<AuthenticatedNodeOwner>,
    ) -> Result<Self::Response, RpcError<Self::Error>> {
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
