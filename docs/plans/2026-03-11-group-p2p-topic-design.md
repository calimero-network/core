# Design: Dedicated Group P2P Topic

**Date:** March 2026
**Status:** Approved
**Replaces:** Context-topic piggybacking for group mutation broadcasts

---

## Problem

Group state synchronization is pull-based. Every node must manually run
`group sync` to discover membership changes, new contexts, capability updates,
visibility changes, etc. The existing `broadcast_group_mutation` mechanism
piggybacks on context P2P topics, which means:

- New members who haven't joined a context yet never receive notifications
- Every broadcast iterates all group contexts (O(N) messages for N contexts)
- Nodes must manually sync after every mutation they didn't initiate

## Decision

Each group gets a dedicated gossipsub topic. Group mutations broadcast on
this topic. All group members subscribe. This replaces (not supplements)
the current context-topic piggybacking.

## Topic Format

```
/calimero/group/<group_id_hex>
```

Mirrors the existing context topic pattern.

## Lifecycle

```
Create Group (Node A)
  → create on-chain
  → subscribe to /calimero/group/<group_id>

Join Group (Node B, via invitation)
  → commit/reveal on-chain
  → subscribe to /calimero/group/<group_id>
  → immediately receives future group mutations

Any Group Mutation (add/remove member, capabilities, visibility, upgrade, etc.)
  → on-chain call
  → broadcast GroupMutationNotification on group topic
  → ALL subscribed group members auto-sync from contract

Remove Member / Leave Group
  → on-chain removal
  → broadcast MembersRemoved on group topic
  → removed node unsubscribes from group topic

Delete Group
  → on-chain deletion
  → broadcast Deleted on group topic
  → all nodes unsubscribe

Node Startup
  → iterate all locally-known groups
  → subscribe to each group topic
  → (mirrors how contexts are subscribed on startup in NodeManager::started)
```

## What Changes

### Simplified broadcast (all 12+ mutation handlers)

Before (current):
```rust
let contexts = group_store::enumerate_group_contexts(&datastore, &group_id, 0, usize::MAX)?;
let _ = node_client
    .broadcast_group_mutation(&contexts, group_id.to_bytes(), GroupMutationKind::MembersAdded)
    .await;
```

After:
```rust
let _ = node_client
    .broadcast_group_mutation(group_id, GroupMutationKind::MembersAdded)
    .await;
```

No more context enumeration in every handler.

### Files to change

| File | Change |
|------|--------|
| **Topic format** | Add `group_topic(group_id) -> TopicHash` alongside existing context topic |
| `node/primitives/src/client.rs` | `broadcast_group_mutation` — publish on group topic (remove context list param). Add `subscribe_group` / `unsubscribe_group` methods. |
| `context/src/handlers/create_group.rs` | After creation → `subscribe_group(group_id)` |
| `context/src/handlers/join_group.rs` | After join → `subscribe_group(group_id)` |
| `context/src/handlers/delete_group.rs` | After delete → `unsubscribe_group(group_id)` |
| `context/src/handlers/remove_group_members.rs` | If self removed → `unsubscribe_group`. Broadcast on group topic. |
| `context/src/handlers/add_group_members.rs` | Remove context enumeration. Broadcast on group topic. |
| `context/src/handlers/remove_group_members.rs` | Remove context enumeration. Broadcast on group topic. |
| `context/src/handlers/upgrade_group.rs` | Remove context enumeration for broadcast (keep for propagator). |
| `context/src/handlers/set_member_capabilities.rs` | Simplify broadcast. |
| `context/src/handlers/set_context_visibility.rs` | Simplify broadcast. |
| `context/src/handlers/manage_context_allowlist.rs` | Simplify broadcast. |
| `context/src/handlers/set_default_capabilities.rs` | Simplify broadcast. |
| `context/src/handlers/set_default_visibility.rs` | Simplify broadcast. |
| `context/src/handlers/update_group_settings.rs` | Simplify broadcast. |
| `context/src/handlers/update_member_role.rs` | Simplify broadcast. |
| `context/src/handlers/detach_context_from_group.rs` | Simplify broadcast. |
| `node/src/lib.rs` | Add group subscription loop in `NodeManager::started` |
| `node/src/handlers/network_event.rs` | Already handles `GroupMutationNotification` → `sync_group()`. No change needed. |

### Existing infrastructure reused (no changes needed)

- `GroupMutationNotification` message type — already defined in `node/primitives/src/sync/snapshot.rs`
- `GroupMutationKind` enum — already has all variants
- Handler in `network_event.rs` lines 287-325 — already calls `sync_group()` on receive
- `sync_group` handler — already syncs metadata + members + contexts from contract

## Deprecation: Admin-Push Add

`group members add` (admin directly adds a member without invitation) will be
deprecated in a future release. The invitation flow (`group invite` + `group
join`) is the proper onboarding path because:

- The joining node runs a handler, which subscribes to the group topic
- The joining node's local store is populated immediately
- No bootstrapping problem (admin-push has no way to notify the added node)

Until removed, admin-push add continues to work but the added node must run
`group sync` once to bootstrap (same as today).

## What Gets Better

| Before | After |
|--------|-------|
| Every mutation requires N-1 manual `group sync` calls | Auto-sync via P2P notification |
| Broadcast iterates all contexts (O(N) messages) | Single message on group topic |
| New members miss notifications until first context join | Notifications from the moment they join the group |
| Every handler has 3 lines of context enumeration boilerplate | Single broadcast call |

## Out of Scope

- **Group state gossip (carrying payload):** Notifications are signals only.
  Receiving nodes sync from the contract. Full state gossip (trusting peer
  data without contract verification) is a future optimization.
- **Real-time delta propagation for group state:** Group state is small and
  changes infrequently. Contract-based sync on notification is sufficient.
- **Removing admin-push add:** Planned separately.
