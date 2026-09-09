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
#[serde(
    tag = "method",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RequestPayload {
    Subscribe(ContextIds),
    Unsubscribe(ContextIds),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextIds {
    /// Default so a group-only subscribe (no `contextIds`) still parses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_ids: Vec<ContextId>,
    /// Groups to observe for group-keyed events (membership, migration).
    /// Hex-encoded, like every id — see `Hash`'s `Display`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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

    // A group-only subscribe (no `contextIds`) with a hex group id must parse.
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

    // The invariant the hex/base58 bug broke: what the admin API emits, what
    // subscribe accepts, and what the event carries are all one hex string.
    #[test]
    fn admin_emit_subscribe_and_event_share_one_hex_representation() {
        use calimero_primitives::events::{
            GroupMembershipEvent, GroupMigrationEvent, GroupMigrationPayload, MembershipChange,
            MembershipChangePayload,
        };

        let id = Hash::from([0x2bu8; 32]);
        // What the admin API emits for this id.
        let emitted = hex::encode(id.as_bytes());

        // Subscribe must deserialize that exact string back into the same id.
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

        // The event carrying that id must serialize `groupId` to the same string.
        let event = GroupMembershipEvent {
            group_id: id,
            payload: MembershipChangePayload::MemberJoined(MembershipChange {
                member: calimero_primitives::identity::AccountId::from([9u8; 32]),
                role: None,
            }),
        };
        let v = serde_json::to_value(&event).expect("event serializes");
        assert_eq!(v["groupId"], emitted, "the event must carry the same hex");

        // Every event keyed on `groupId` is pinned to that one representation,
        // not just the first one that was.
        let migration = GroupMigrationEvent {
            group_id: id,
            payload: GroupMigrationPayload::MigrationCompleted {
                to_version: "10.2.0".to_owned(),
                completed_at: 1_700_000_000,
            },
        };
        let v = serde_json::to_value(&migration).expect("event serializes");
        assert_eq!(
            v["groupId"], emitted,
            "the migration event must carry the same hex"
        );
    }

    // A context-only subscribe (no `groupIds`) still parses.
    #[test]
    fn context_only_subscribe_parses() {
        match parse_payload(
            r#"{"id":"1","method":"subscribe","params":{"contextIds":["0000000000000000000000000000000000000000000000000000000000000000"]}}"#,
        ) {
            RequestPayload::Subscribe(ids) => {
                assert_eq!(ids.context_ids.len(), 1);
                assert!(ids.group_ids.is_empty());
            }
            other => panic!("expected Subscribe, got {other:?}"),
        }
    }
}
