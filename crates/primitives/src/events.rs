use serde::{Deserialize, Serialize};

use crate::context::ContextId;
use crate::hash::Hash;
use crate::sync_status::SyncState;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum NodeEvent {
    Context(ContextEvent),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEvent {
    pub context_id: ContextId,
    #[serde(flatten)]
    pub payload: ContextEventPayload,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "PascalCase")]
#[allow(variant_size_differences, reason = "fine for now")]
pub enum ContextEventPayload {
    StateMutation(StateMutationPayload),
    /// Live sync-status update, pushed to subscribers as the sync run-loop
    /// changes phase (and as snapshot pages arrive). Lets a client waiting on
    /// initial state watch progress instead of polling `sync_status`.
    SyncStatus(SyncStatusPayload),
    /// Fired once when a context's application version flips (a migrate/upgrade
    /// applied). Lets a frontend react live to bundle skew (spec skew #2)
    /// instead of polling. `contextId` rides on the flattened [`ContextEvent`].
    AppVersionChanged(AppVersionChangedPayload),
    /// Emitted once per cross-context call dispatched from a source execution —
    /// on success, denial (an L1/L3 gate), or target execution error. Gives the
    /// fire-and-forget xcall path an out-of-band feedback channel (#2137).
    /// `contextId` on the wrapper is the *source* context.
    XCall(XCallPayload),
}

/// Payload of a [`ContextEventPayload::AppVersionChanged`] event. Versions are
/// the application semver before/after the flip; either may be `None` if the
/// corresponding `ApplicationMeta` row was unavailable at emit time.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppVersionChangedPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_version: Option<String>,
}

/// Payload of a [`ContextEventPayload::SyncStatus`] event. Mirrors the fields
/// of the `sync_status` JSON-RPC response that the run-loop knows; `is_initialized`
/// is deliberately omitted (it's a context-layer fact, not a sync-phase one —
/// a client reads it from the RPC or infers initialization from the first
/// [`ContextEventPayload::StateMutation`]).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusPayload {
    pub sync_state: SyncState,
    pub failure_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateMutationPayload {
    pub new_root: Hash,
    pub events: Option<Vec<ExecutionEvent>>,
}

impl StateMutationPayload {
    #[must_use]
    pub const fn with_root_and_events(new_root: Hash, events: Vec<ExecutionEvent>) -> Self {
        Self {
            new_root,
            events: Some(events),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecutionEvent {
    pub kind: String,
    pub data: Vec<u8>,
    pub handler: Option<String>,
}

/// Payload of a [`ContextEventPayload::XCall`] event. `contextId` on the
/// flattened [`ContextEvent`] is the *source* context; `targetContextId` is
/// the callee. Emitted on success, denial, or target execution error.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XCallPayload {
    pub target_context_id: ContextId,
    pub function: String,
    pub outcome: XCallOutcome,
}

/// Result of an attempted cross-context call.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum XCallOutcome {
    /// Dispatched and the target execution returned `Ok`.
    Ok,
    /// Refused before dispatch by a node-enforced gate (L1 namespace boundary,
    /// L3 entry-point check, or no owned member of the target).
    Denied { reason: String },
    /// Dispatched but the target execution returned an error.
    ExecError { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    // AppVersionChanged serializes with the PascalCase "AppVersionChanged" tag
    // and camelCase data fields; contextId rides on the flattened ContextEvent.
    #[test]
    fn app_version_changed_tag_and_shape() {
        let event = ContextEvent {
            context_id: ContextId::from([0x01; 32]),
            payload: ContextEventPayload::AppVersionChanged(AppVersionChangedPayload {
                from_version: Some("1.0.0".to_owned()),
                to_version: Some("2.0.0".to_owned()),
            }),
        };
        let v = serde_json::to_value(&event).expect("serialize");
        assert_eq!(v["type"], "AppVersionChanged");
        assert_eq!(v["data"]["fromVersion"], "1.0.0");
        assert_eq!(v["data"]["toVersion"], "2.0.0");
        assert!(v.get("contextId").is_some(), "contextId on the wrapper");
    }

    // None versions are omitted from the data object.
    #[test]
    fn app_version_changed_omits_none() {
        let payload = ContextEventPayload::AppVersionChanged(AppVersionChangedPayload {
            from_version: None,
            to_version: Some("2.0.0".to_owned()),
        });
        let v = serde_json::to_value(&payload).expect("serialize");
        assert!(v["data"].get("fromVersion").is_none());
        assert_eq!(v["data"]["toVersion"], "2.0.0");
    }

    // XCall events carry the source on the wrapper (contextId), the callee +
    // function in data, and a tagged outcome. Denied carries a reason.
    #[test]
    fn xcall_event_serializes_with_outcome() {
        let event = ContextEvent {
            context_id: ContextId::from([0x02; 32]),
            payload: ContextEventPayload::XCall(XCallPayload {
                target_context_id: ContextId::from([0x03; 32]),
                function: "on_match_finished".to_owned(),
                outcome: XCallOutcome::Denied {
                    reason: "namespace boundary".to_owned(),
                },
            }),
        };
        let v = serde_json::to_value(&event).expect("serialize");
        assert_eq!(v["type"], "XCall");
        assert_eq!(v["data"]["function"], "on_match_finished");
        assert_eq!(v["data"]["outcome"]["status"], "denied");
        assert_eq!(
            v["data"]["outcome"]["detail"]["reason"],
            "namespace boundary"
        );
        assert!(
            v.get("contextId").is_some(),
            "source contextId on the wrapper"
        );

        // round-trips
        let json = serde_json::to_string(&event).expect("to_string");
        let back: ContextEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.context_id, event.context_id);
    }

    // The Ok outcome is a bare tagged variant (no detail object).
    #[test]
    fn xcall_outcome_ok_shape() {
        let v = serde_json::to_value(XCallOutcome::Ok).expect("serialize");
        assert_eq!(v["status"], "ok");
    }
}
