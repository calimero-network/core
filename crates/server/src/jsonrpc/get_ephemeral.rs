//! JSON-RPC `get_ephemeral` handler.
//!
//! Returns a snapshot of live ephemeral-presence entries for a context from
//! the node's in-memory [`AwarenessStore`]. The snapshot is the client's
//! initial seed; live deltas arrive over the SSE/WS event stream.

use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{
    EphemeralEntry, GetEphemeralError, GetEphemeralRequest, GetEphemeralResponse,
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

        Ok(GetEphemeralResponse::new(
            entries
                .into_iter()
                .map(|(author, state)| EphemeralEntry::new(author, state))
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
        EphemeralEntry, GetEphemeralRequest, GetEphemeralResponse, Request, RequestId,
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
        let resp =
            GetEphemeralResponse::new(vec![EphemeralEntry::new(author, state_bytes.clone())]);
        let json_str = serde_json::to_string(&resp).expect("serialize");
        let decoded: GetEphemeralResponse = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(decoded.entries.len(), 1);
        assert_eq!(decoded.entries[0].author, author);
        assert_eq!(decoded.entries[0].state, state_bytes);
    }

    #[test]
    fn get_ephemeral_response_entries_field_is_camel_case_and_present() {
        let resp = GetEphemeralResponse::new(vec![]);
        let json_val: Value = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(json_val, json!({ "entries": [] }));
    }

    #[test]
    fn ephemeral_entry_author_and_state_field_names() {
        let author = PublicKey::from([0x11; 32]);
        let entry = EphemeralEntry::new(author, vec![5, 6]);
        let json_val: Value = serde_json::to_value(&entry).expect("serialize");
        assert!(
            json_val.get("author").is_some(),
            "author field must be present"
        );
        assert!(
            json_val.get("state").is_some(),
            "state field must be present"
        );
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
