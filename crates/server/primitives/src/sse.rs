use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use eyre::Error as EyreError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Client ID is a locally unique identifier of a SSE client connection.
pub type ConnectionId = u64;

#[derive(Debug)]
pub enum Command {
    Close(String),
    Send(Response),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request<P> {
    pub id: String,
    #[serde(flatten)]
    pub payload: P,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RequestPayload {
    Subscribe(ContextIds),
    Unsubscribe(ContextIds),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextIds {
    /// Default so a group-only subscribe (`groupIds` present, `contextIds`
    /// omitted) parses; without it such a payload fails with "missing field
    /// `contextIds`" and no subscription is registered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_ids: Vec<ContextId>,
    /// Group ids to observe for `GroupMembership` events. Optional, so
    /// existing `contextIds`-only clients are unaffected. Hex-encoded, matching
    /// the group/namespace admin API's id representation (not base58).
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        with = "calimero_primitives::hash::hex_repr::vec"
    )]
    pub group_ids: Vec<Hash>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    #[serde(flatten)]
    pub body: ResponseBody,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::exhaustive_enums,
    reason = "This will only ever have these variants"
)]
pub enum ResponseBody {
    Result(Value),
    Error(ResponseBodyError),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ResponseBodyError {
    ServerError(ServerResponseError),
    HandlerError(Value),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerResponseError {
    ParseError(String),
    InternalError {
        #[serde(skip)]
        err: Option<EyreError>,
    },
}

#[derive(Debug, Copy, Clone)]
pub enum SseEvent {
    Message,
    Close,
    Error,
    Connect,
}

impl SseEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            SseEvent::Message => "message",
            SseEvent::Close => "close",
            SseEvent::Error => "error",
            SseEvent::Connect => "connect",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_payload(raw: &str) -> RequestPayload {
        let req: Request<serde_json::Value> = serde_json::from_str(raw).expect("envelope parses");
        serde_json::from_value(req.payload).expect("subscribe payload parses")
    }

    // A group-only subscribe carrying the exact payload the mero-js SSE client
    // sends: `contextIds` omitted, and `groupIds` a HEX-encoded 32-byte id (the
    // representation the group/namespace admin API returns). Both must parse, or
    // no subscription is registered and the events are dropped.
    #[test]
    fn group_only_hex_subscribe_parses() {
        let hex = "0d477ad6039aec8f2e7fc72a7f23aec1b98a08ba7c2faac7ba5271954813509a";
        match parse_payload(&format!(
            r#"{{"id":"1","method":"subscribe","params":{{"groupIds":["{hex}"]}}}}"#
        )) {
            RequestPayload::Subscribe(ids) => {
                assert!(ids.context_ids.is_empty());
                assert_eq!(ids.group_ids.len(), 1);
                assert_eq!(hex::encode(ids.group_ids[0].as_bytes()), hex);
            }
            other => panic!("expected Subscribe, got {other:?}"),
        }
    }

    // The invariant the hex/base58 bug broke: the id representation the
    // group/namespace admin API EMITS must be exactly the one the subscribe
    // payload ACCEPTS and the one the emitted event CARRIES. If any leg drifts
    // (e.g. subscribe back to base58), a client can't correlate what it holds,
    // subscribes with, and receives. All three are pinned to one hex string.
    #[test]
    fn admin_emit_subscribe_and_event_share_one_hex_representation() {
        use calimero_primitives::events::{
            GroupMembershipEvent, MembershipChange, MembershipChangePayload,
        };
        use calimero_primitives::identity::PublicKey;

        let id = Hash::from([0x2bu8; 32]);
        // Leg 1: what the admin API emits for this id.
        let emitted = hex::encode(id.as_bytes());

        // Leg 2: the subscribe payload must deserialize that exact string back
        // into the same id.
        let parsed = match parse_payload(&format!(
            r#"{{"id":"1","method":"subscribe","params":{{"groupIds":["{emitted}"]}}}}"#
        )) {
            RequestPayload::Subscribe(ids) => ids,
            other => panic!("expected Subscribe, got {other:?}"),
        };
        assert_eq!(
            parsed.group_ids,
            vec![id],
            "subscribe must accept the emitted hex"
        );

        // Leg 3: the event carrying that id must serialize `groupId` to the
        // same string.
        let event = GroupMembershipEvent {
            group_id: id,
            payload: MembershipChangePayload::MemberJoined(MembershipChange {
                member: PublicKey::from([9u8; 32]),
                role: None,
            }),
        };
        let v = serde_json::to_value(&event).expect("event serializes");
        assert_eq!(v["groupId"], emitted, "the event must carry the same hex");
    }

    // A context-only subscribe (legacy clients) still parses with `groupIds`
    // absent.
    #[test]
    fn context_only_subscribe_parses() {
        match parse_payload(
            r#"{"id":"1","method":"subscribe","params":{"contextIds":["11111111111111111111111111111111"]}}"#,
        ) {
            RequestPayload::Subscribe(ids) => {
                assert_eq!(ids.context_ids.len(), 1);
                assert!(ids.group_ids.is_empty());
            }
            other => panic!("expected Subscribe, got {other:?}"),
        }
    }
}
