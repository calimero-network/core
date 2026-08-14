use serde::{Deserialize, Serialize};

use crate::context::{ContextId, GroupMemberRole};
use crate::hash::Hash;
use crate::identity::PublicKey;
use crate::sync_status::SyncState;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum NodeEvent {
    Context(ContextEvent),
    /// A group's membership changed (join/add/remove/leave). Keyed by `groupId`,
    /// disjoint from `contextId`, so untagged still round-trips.
    GroupMembership(GroupMembershipEvent),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMembershipEvent {
    /// The group whose membership changed (the joined subgroup, never the
    /// namespace root). Hex-encoded to match the group/namespace admin API.
    #[serde(with = "crate::hash::hex_repr")]
    pub group_id: Hash,
    #[serde(flatten)]
    pub payload: MembershipChangePayload,
}

/// The kind of membership change, tagged like [`ContextEventPayload`]. `MemberLeft`
/// and an admin `MemberRemoved` both surface as `MemberRemoved`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "PascalCase")]
pub enum MembershipChangePayload {
    /// A member joined via a self-service path (open invite or inherited
    /// Open-subgroup join).
    MemberJoined(MembershipChange),
    /// An admin added a member.
    MemberAdded(MembershipChange),
    /// A member was removed or left the group.
    MemberRemoved(MembershipChange),
}

/// The affected identity and, when known, the role it holds in the group.
/// `role` is absent for an inherited Open-subgroup join and for removals.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MembershipChange {
    /// The member's ACCOUNT — the principal the governance rows name.
    ///
    /// Named `memberAccount` on the wire, not `member`. The field it replaces
    /// carried a bs58 signing KEY; this carries 64-hex naming a different
    /// principal entirely. Both are strings, so a consumer still reading
    /// `member` would have parsed the new value as the old kind and compared it
    /// against keys forever — silently, and against nothing. Renaming makes a
    /// consumer that has not been updated fail at the field it no longer finds,
    /// which is the only honest way to ship this change.
    #[serde(rename = "memberAccount")]
    pub member: crate::identity::AccountId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<GroupMemberRole>,
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
    /// Emitted once per cross-context call — on success, denial, or target
    /// execution error — giving the fire-and-forget xcall path a feedback
    /// channel (#2137). `contextId` on the wrapper is the *source* context.
    XCall(XCallPayload),
    /// Transient per-peer presence update; never persisted, decrypted from the
    /// context group key before reaching a subscriber.
    Ephemeral(EphemeralPayload),
}

/// Maximum size, in bytes, of a single ephemeral-presence slice.
///
/// **Single source of truth** for the presence size cap, shared across the
/// node (which enforces it on the outbound path in
/// `calimero-node::handlers::ephemeral`) and the JSON-RPC layer (which
/// pre-validates against it in `calimero-server`'s `set_ephemeral` handler so
/// the client receives a typed `SliceTooLarge` error). Defined here — in the
/// crate both depend on — so the two paths can never drift.
pub const EPHEMERAL_MAX_BYTES: usize = 16_384;

/// Payload of a [`ContextEventPayload::Ephemeral`] event. Carries a per-peer
/// presence slice, decrypted from the context group key before delivery.
/// `state` is present on upsert and absent on TTL/disconnect expiry
/// (`removed = true`). `contextId` rides on the flattened [`ContextEvent`].
///
/// # Security — `author` is cryptographically authenticated
///
/// The presence envelope is encrypted under the context **group key** and
/// **signed** by `author`'s identity key over `(context_id, author, seq,
/// key_id, sha256(ciphertext))` (`calimero-node`'s
/// `handlers::ephemeral::auth`). The receive path verifies this signature
/// before the slice reaches the awareness store or this event is emitted, so
/// a group-key holder cannot forge another member's `author`. A message that
/// fails verification is silently dropped and never surfaces as an event.
/// Presence remains transient and never persisted, but by the time clients
/// see `author` here it has been authenticated the same way a state-delta
/// author is.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EphemeralPayload {
    /// The peer whose presence slice this update belongs to. Verified against
    /// an ed25519 signature on receipt — see the type-level security note.
    pub author: PublicKey,
    /// Decrypted slice bytes on upsert; absent when `removed` is `true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Vec<u8>>,
    /// `true` on TTL/disconnect expiry; `state` is omitted in that case.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
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
    /// Refused before dispatch (wrong namespace, not an `#[app::xcall]` entry
    /// point, or no owned member of the target). `reason` says which.
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
                    reason: "owning group boundary".to_owned(),
                },
            }),
        };
        let v = serde_json::to_value(&event).expect("serialize");
        assert_eq!(v["type"], "XCall");
        assert_eq!(v["data"]["function"], "on_match_finished");
        assert_eq!(v["data"]["outcome"]["status"], "denied");
        assert_eq!(
            v["data"]["outcome"]["detail"]["reason"],
            "owning group boundary"
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

    // groupId on the wrapper, type tag + member/role in data; role omitted when absent.
    #[test]
    fn group_membership_tag_and_shape() {
        let event = NodeEvent::GroupMembership(GroupMembershipEvent {
            group_id: Hash::from([0x07; 32]),
            payload: MembershipChangePayload::MemberJoined(MembershipChange {
                member: crate::identity::AccountId::from([0x09; 32]),
                role: Some(GroupMemberRole::Member),
            }),
        });
        let v = serde_json::to_value(&event).expect("serialize");
        assert_eq!(v["type"], "MemberJoined");
        // groupId must be hex (matches the group/namespace admin API), so a client
        // can correlate the event to the id it subscribed with.
        assert_eq!(v["groupId"], hex::encode([0x07; 32]), "groupId is hex");
        assert!(v.get("contextId").is_none(), "no contextId leaks in");
        assert_eq!(v["data"]["role"], "Member");
        // Named `memberAccount`, and `member` must NOT be present. The old
        // field carried a bs58 key; this carries a 64-hex account. Both are
        // strings, so a consumer left reading `member` would have compared an
        // account against keys and matched nothing, forever and silently. The
        // absence assertion is the one that keeps the rename honest.
        assert_eq!(
            v["data"]["memberAccount"],
            hex::encode([0x09; 32]),
            "the member is named by account, in hex"
        );
        assert!(
            v["data"].get("member").is_none(),
            "the old key-shaped field must be gone, not shadowed — a consumer that \
             still reads it should fail loudly rather than silently mismatch"
        );
    }

    #[test]
    fn group_membership_omits_role_when_absent() {
        let v = serde_json::to_value(MembershipChangePayload::MemberRemoved(MembershipChange {
            member: crate::identity::AccountId::from([0x0A; 32]),
            role: None,
        }))
        .expect("serialize");
        assert_eq!(v["type"], "MemberRemoved");
        assert!(v["data"].get("role").is_none(), "None role omitted");
    }

    // The untagged NodeEvent still round-trips with a second variant.
    #[test]
    fn node_event_untagged_round_trips_both_variants() {
        let group = NodeEvent::GroupMembership(GroupMembershipEvent {
            group_id: Hash::from([0x11; 32]),
            payload: MembershipChangePayload::MemberAdded(MembershipChange {
                member: crate::identity::AccountId::from([0x12; 32]),
                role: Some(GroupMemberRole::Admin),
            }),
        });
        let json = serde_json::to_string(&group).expect("to_string");
        let back: NodeEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(
            matches!(back, NodeEvent::GroupMembership(_)),
            "got {back:?}"
        );

        let ctx = NodeEvent::Context(ContextEvent {
            context_id: ContextId::from([0x13; 32]),
            payload: ContextEventPayload::AppVersionChanged(AppVersionChangedPayload {
                from_version: None,
                to_version: Some("2.0.0".to_owned()),
            }),
        });
        let json = serde_json::to_string(&ctx).expect("to_string");
        let back: NodeEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(back, NodeEvent::Context(_)), "got {back:?}");
    }

    // Ephemeral upsert: tag is "Ephemeral", state present, contextId on wrapper.
    #[test]
    fn ephemeral_upsert_tag_and_shape() {
        let event = ContextEvent {
            context_id: ContextId::from([0x01; 32]),
            payload: ContextEventPayload::Ephemeral(EphemeralPayload {
                author: PublicKey::from([0x05; 32]),
                state: Some(vec![1, 2, 3]),
                removed: false,
            }),
        };
        let v = serde_json::to_value(&event).expect("serialize");
        assert_eq!(v["type"], "Ephemeral");
        assert_eq!(v["data"]["state"], serde_json::json!([1, 2, 3]));
        assert!(v.get("contextId").is_some());
    }

    // Ephemeral removal: state absent, removed=true.
    #[test]
    fn ephemeral_removed_omits_state() {
        let payload = ContextEventPayload::Ephemeral(EphemeralPayload {
            author: PublicKey::from([0x05; 32]),
            state: None,
            removed: true,
        });
        let v = serde_json::to_value(&payload).expect("serialize");
        assert!(v["data"].get("state").is_none());
        assert_eq!(v["data"]["removed"], true);
    }
}
