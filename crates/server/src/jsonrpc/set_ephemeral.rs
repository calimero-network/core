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

        // Delegate to the node actor, which enforces EPHEMERAL_MAX_BYTES and
        // drives the seq counter + async publish.
        state
            .node_client
            .set_local_ephemeral(context_id, author, self.state)
            .await
            .map_err(|err| {
                // The node returns a typed `EphemeralOutboundError::SliceTooLarge(n)`
                // wrapped in an eyre report. Since the node crate is not available
                // here we detect it by the error message the typed error formats to
                // ("ephemeral slice is too large"). Both size and generic errors map
                // through `InternalError(msg)` so the client always gets a readable
                // string; a future split can add a `SliceTooLarge` wire variant.
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
}
