use serde::{Deserialize, Serialize};

use crate::context::{ContextId, GroupMemberRole};
use crate::hash::Hash;
use crate::sync_status::SyncState;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum NodeEvent {
    Context(ContextEvent),
    /// A group's membership changed (join/add/remove/leave). Keyed by `groupId`,
    /// disjoint from `contextId`, so untagged still round-trips.
    GroupMembership(GroupMembershipEvent),
    /// A namespace migration changed phase. Keyed by `groupId` like
    /// [`GroupMembershipEvent`]; the payload tags are disjoint from that enum's.
    GroupMigration(GroupMigrationEvent),
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

/// Shaped like [`GroupMembershipEvent`] but not authorized like it: delivery
/// runs the group-subscription filter AND the per-variant
/// [`GroupMigrationPayload::requires_group_admin`] branch, so a new variant
/// carrying identities must declare its own gate there.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMigrationEvent {
    /// The namespace root the migration runs under, never a descendant
    /// subgroup: it is the id a client subscribes with. Hex, like `groupId`
    /// everywhere else.
    #[serde(with = "crate::hash::hex_repr")]
    pub group_id: Hash,
    #[serde(flatten)]
    pub payload: GroupMigrationPayload,
}

/// The phase a migration reached, tagged like [`MembershipChangePayload`]. The
/// per-variant `rename_all` is load-bearing: the enum-level one renames the
/// variants, not their fields.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "PascalCase")]
pub enum GroupMigrationPayload {
    /// A migration was accepted and its target ABI state version resolved.
    /// `local_contexts_total` is the emitting node's own context count, not a
    /// fleet number - distinct from `MigrationProgress.total`, the cohort size.
    #[serde(rename_all = "camelCase")]
    MigrationStarted {
        from_version: String,
        to_version: String,
        to_state_version: u32,
        local_contexts_total: u32,
    },
    /// Fleet rollup counters, recomputed when a peer's heartbeat facts change.
    /// Counters only - the per-member detail behind them stays admin-only, in
    /// the `migration-status` read a plain member cannot call.
    #[serde(rename_all = "camelCase")]
    MigrationProgress {
        migrated: usize,
        in_progress: usize,
        unknown: usize,
        failed: usize,
        total: usize,
    },
    /// One subgroup's local context swaps advanced on the emitting node. Named
    /// like [`Self::MigrationStarted`]'s counter and for the same reason: the
    /// sibling `MigrationProgress.total` is the cohort size, and a subscriber
    /// reading a bare `total` off this stream cannot tell the two apart.
    #[serde(rename_all = "camelCase")]
    CascadeProgress {
        #[serde(with = "crate::hash::hex_repr")]
        subgroup_id: Hash,
        local_contexts_swapped: u32,
        local_contexts_total: u32,
    },
    /// Every pinned-cohort member reported the target. This is the moment
    /// `completed_at` becomes knowable under lazy upgrades.
    #[serde(rename_all = "camelCase")]
    MigrationCompleted {
        to_version: String,
        completed_at: u64,
    },
}

impl GroupMigrationPayload {
    /// Whether this payload may only be delivered to an admin of the namespace
    /// root the event is keyed on.
    ///
    /// `CascadeProgress` names a descendant `subgroup_id`, and a Restricted
    /// subgroup's existence is not something a plain namespace member may
    /// enumerate - the same disclosure the `migration-status` read is
    /// admin-gated against. The other variants carry counters only.
    #[must_use]
    pub const fn requires_group_admin(&self) -> bool {
        matches!(*self, Self::CascadeProgress { .. })
    }
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

    // groupId on the wrapper (hex, matching the id a client subscribes with),
    // PascalCase type tag, camelCase data fields.
    #[test]
    fn group_migration_tag_and_shape() {
        let event = NodeEvent::GroupMigration(GroupMigrationEvent {
            group_id: Hash::from([0x21; 32]),
            payload: GroupMigrationPayload::MigrationStarted {
                from_version: "10.1.3".to_owned(),
                to_version: "10.2.0".to_owned(),
                to_state_version: 2,
                local_contexts_total: 7,
            },
        });
        let v = serde_json::to_value(&event).expect("serialize");
        assert_eq!(v["type"], "MigrationStarted");
        assert_eq!(v["groupId"], hex::encode([0x21; 32]), "groupId is hex");
        assert!(v.get("contextId").is_none(), "no contextId leaks in");
        assert_eq!(v["data"]["fromVersion"], "10.1.3");
        assert_eq!(v["data"]["toStateVersion"], 2);
        assert_eq!(v["data"]["localContextsTotal"], 7);
    }

    #[test]
    fn cascade_progress_carries_a_hex_subgroup_id() {
        let v = serde_json::to_value(GroupMigrationPayload::CascadeProgress {
            subgroup_id: Hash::from([0x31; 32]),
            local_contexts_swapped: 3,
            local_contexts_total: 5,
        })
        .expect("serialize");
        assert_eq!(v["type"], "CascadeProgress");
        assert_eq!(v["data"]["subgroupId"], hex::encode([0x31; 32]));
        assert_eq!(v["data"]["localContextsSwapped"], 3);
        assert_eq!(
            v["data"]["localContextsTotal"], 5,
            "node-local, never the cohort total the sibling variant carries"
        );
        assert!(
            v["data"].get("total").is_none(),
            "a bare `total` here would collide with MigrationProgress's cohort size"
        );
    }

    // The untagged NodeEvent resolves each variant by its payload tag. The two
    // group-keyed variants are the hazard: a payload tag shared between them
    // would let the first-declared one swallow the other's events.
    #[test]
    fn node_event_untagged_round_trips_every_variant() {
        let migration = NodeEvent::GroupMigration(GroupMigrationEvent {
            group_id: Hash::from([0x41; 32]),
            payload: GroupMigrationPayload::MigrationCompleted {
                to_version: "10.2.0".to_owned(),
                completed_at: 1_700_000_000,
            },
        });
        let json = serde_json::to_string(&migration).expect("to_string");
        let back: NodeEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(back, NodeEvent::GroupMigration(_)), "got {back:?}");

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
}
