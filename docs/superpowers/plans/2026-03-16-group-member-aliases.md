# Group Member Aliases Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add propagated per-group member aliases so each group member can set a display name scoped to a specific group that peers receive via gossip.

**Architecture:** Mirror the existing `ContextAliasSet` pattern exactly — store aliases in a new RocksDB key space (`0x2D`), gossip them with a new `MemberAliasSet` `GroupMutationKind` variant, re-broadcast all member aliases when a peer subscribes, and receive+apply them in `network_event.rs`. Authorization is self-only: a member may only alias themselves.

**Tech Stack:** Rust 1.88, actix actors, Borsh, RocksDB (calimero-store), axum (REST layer)

---

## File Structure

| File | Created / Modified | Responsibility |
|---|---|---|
| `crates/store/src/key/group.rs` | Modify | Add `GroupMemberAlias` key type (prefix `0x2D`) |
| `crates/context/src/group_store.rs` | Modify | Add `set_member_alias`, `get_member_alias`, `enumerate_member_aliases` |
| `crates/node/primitives/src/sync/snapshot.rs` | Modify | Append `MemberAliasSet` variant (discriminant 15) to `GroupMutationKind` |
| `crates/context/primitives/src/group.rs` | Modify | Add `alias` to `GroupMemberEntry`; add `SetMemberAliasRequest`, `StoreMemberAliasRequest` |
| `crates/context/primitives/src/messages.rs` | Modify | Add `SetMemberAlias`, `StoreMemberAlias` variants + imports |
| `crates/context/primitives/src/client.rs` | Modify | Add `set_member_alias`, `store_member_alias` async methods |
| `crates/context/src/handlers.rs` | Modify | Add `set_member_alias` + `store_member_alias` module declarations + dispatch arms |
| `crates/context/src/handlers/set_member_alias.rs` | Create | Async handler: auth check → local write → gossip |
| `crates/context/src/handlers/store_member_alias.rs` | Create | Sync handler: write alias received from gossip |
| `crates/context/src/handlers/list_group_members.rs` | Modify | Include alias in each `GroupMemberEntry` |
| `crates/context/src/handlers/broadcast_group_local_state.rs` | Modify | Enumerate + broadcast `MemberAliasSet` variants on subscription |
| `crates/node/src/handlers/network_event.rs` | Modify | Handle `MemberAliasSet` gossip variant → call `store_member_alias` |
| `crates/server/primitives/src/admin.rs` | Modify | Add `alias` to `GroupMemberApiEntry`; add `SetMemberAliasApiRequest/Response` types |
| `crates/server/src/admin/handlers/groups/set_member_alias.rs` | Create | REST handler for `PUT /admin-api/groups/:group_id/members/:identity/alias` |
| `crates/server/src/admin/handlers/groups.rs` | Modify | Add `pub mod set_member_alias;` |
| `crates/server/src/admin/service.rs` | Modify | Add route `PUT /groups/:group_id/members/:identity/alias` |

---

## Chunk 1: Storage Layer

### Task 1: New RocksDB key type for group member alias

**Files:**
- Modify: `crates/store/src/key/group.rs`

- [ ] **Step 1: Read `crates/store/src/key/group.rs` and note the last defined prefix (`0x2C = GROUP_CONTEXT_ALIAS_PREFIX`)**

- [ ] **Step 2: Add `GROUP_MEMBER_ALIAS_PREFIX` constant and `GroupMemberAlias` key struct**

After the `GroupContextAlias` implementation block (around line 821), add:

```rust
pub const GROUP_MEMBER_ALIAS_PREFIX: u8 = 0x2D;

/// Stores a human-readable alias for a group member scoped to a specific group.
/// Key: prefix (1 byte) + group_id (32 bytes) + member_pk (32 bytes) → alias String
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GroupMemberAlias(Key<(GroupPrefix, GroupIdComponent, GroupIdComponent)>);

impl GroupMemberAlias {
    #[must_use]
    pub fn new(group_id: [u8; 32], member: PrimitivePublicKey) -> Self {
        Self(Key(GenericArray::from([GROUP_MEMBER_ALIAS_PREFIX])
            .concat(GenericArray::from(group_id))
            .concat(GenericArray::from(*member))))
    }

    #[must_use]
    pub fn group_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn member(&self) -> PrimitivePublicKey {
        let mut pk = [0; 32];
        pk.copy_from_slice(&AsRef::<[_; 65]>::as_ref(&self.0)[33..]);
        pk.into()
    }
}

impl AsKeyParts for GroupMemberAlias {
    type Components = (GroupPrefix, GroupIdComponent, GroupIdComponent);

    fn column() -> Column {
        Column::Group
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for GroupMemberAlias {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for GroupMemberAlias {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupMemberAlias")
            .field("group_id", &self.group_id())
            .field("member", &self.member())
            .finish()
    }
}
```

- [ ] **Step 3: Add `GROUP_MEMBER_ALIAS_PREFIX` to the `distinct_prefixes` test array**

In the `distinct_prefixes` test (around line 938), add `GROUP_MEMBER_ALIAS_PREFIX` to the `prefixes` array.

- [ ] **Step 4: Add roundtrip test**

```rust
#[test]
fn group_member_alias_roundtrip() {
    let gid = [0xDA; 32];
    let pk = PrimitivePublicKey::from([0xDB; 32]);
    let key = GroupMemberAlias::new(gid, pk);
    assert_eq!(key.group_id(), gid);
    assert_eq!(key.member(), pk);
    assert_eq!(key.as_key().as_bytes()[0], GROUP_MEMBER_ALIAS_PREFIX);
    assert_eq!(key.as_key().as_bytes().len(), 65);
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p calimero-store -- --test-output immediate 2>&1 | tail -20
```
Expected: all `group.rs` tests pass, including `distinct_prefixes` and `group_member_alias_roundtrip`.

- [ ] **Step 6: Commit**

```bash
git add crates/store/src/key/group.rs
git commit -m "feat(store): add GroupMemberAlias key type (prefix 0x2D)"
```

---

### Task 2: group_store helpers

**Files:**
- Modify: `crates/context/src/group_store.rs`

- [ ] **Step 1: Add `GroupMemberAlias` and `GROUP_MEMBER_ALIAS_PREFIX` to the `calimero_store::key` import**

The existing import block starts at line 10. Add `GroupMemberAlias` and `GROUP_MEMBER_ALIAS_PREFIX` to it.

- [ ] **Step 2: Add `set_member_alias` and `get_member_alias` functions**

Add after `set_context_alias` / `get_context_alias` functions (around line 533):

```rust
/// Stores a human-readable alias for a group member within a group.
pub fn set_member_alias(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    alias: &str,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(
        &GroupMemberAlias::new(group_id.to_bytes(), *member),
        &alias.to_owned(),
    )?;
    Ok(())
}

/// Returns the alias for a group member within a group, if one was set.
pub fn get_member_alias(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Option<String>> {
    let handle = store.handle();
    handle
        .get(&GroupMemberAlias::new(group_id.to_bytes(), *member))
        .map_err(Into::into)
}
```

- [ ] **Step 3: Add `enumerate_member_aliases` function**

Add after `enumerate_group_contexts_with_aliases` (around line 549). Uses the `iter.seek` + boundary-check pattern from `list_group_members`:

```rust
/// Returns all member aliases stored for a group as `(PublicKey, alias_string)` pairs.
pub fn enumerate_member_aliases(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(PublicKey, String)>> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupMemberAlias::new(group_id_bytes, PublicKey::from([0u8; 32]));
    let mut iter = handle.iter::<GroupMemberAlias>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;
        if key.as_key().as_bytes()[0] != GROUP_MEMBER_ALIAS_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        let Some(alias) = handle.get(&key)? else {
            continue;
        };
        results.push((key.member(), alias));
    }

    Ok(results)
}
```

- [ ] **Step 4: Run cargo check**

```bash
cargo check -p calimero-context 2>&1 | grep -E "^error" | head -20
```
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/context/src/group_store.rs
git commit -m "feat(context): add member alias store helpers (set/get/enumerate)"
```

---

## Chunk 2: Wire Protocol + Actor Infrastructure

### Task 3: New GroupMutationKind variant

**Files:**
- Modify: `crates/node/primitives/src/sync/snapshot.rs`

- [ ] **Step 1: Read the `GroupMutationKind` enum (around line 646–683)**

Confirm the last variant is `ContextAllowlistSet` (discriminant 14).

- [ ] **Step 2: Append `MemberAliasSet` as discriminant 15**

Add immediately after the closing `}` of `ContextAllowlistSet`:

```rust
    MemberAliasSet {
        member: [u8; 32],
        alias: String,
    },
```

**Critical**: Do NOT reorder existing variants — Borsh discriminants are positional.

- [ ] **Step 3: Run Borsh discriminant regression test**

```bash
cargo test -p calimero-node-primitives 2>&1 | tail -20
```
Expected: all existing tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/node/primitives/src/sync/snapshot.rs
git commit -m "feat(node-primitives): add MemberAliasSet GroupMutationKind variant (discriminant 15)"
```

---

### Task 4: Actor message structs

**Files:**
- Modify: `crates/context/primitives/src/group.rs`

- [ ] **Step 1: Add `alias` field to `GroupMemberEntry`**

Change:
```rust
#[derive(Clone, Debug)]
pub struct GroupMemberEntry {
    pub identity: PublicKey,
    pub role: GroupMemberRole,
}
```
To:
```rust
#[derive(Clone, Debug)]
pub struct GroupMemberEntry {
    pub identity: PublicKey,
    pub role: GroupMemberRole,
    pub alias: Option<String>,
}
```

- [ ] **Step 2: Add `SetMemberAliasRequest` and `StoreMemberAliasRequest`**

Add after `BroadcastGroupLocalStateRequest` (around line 486):

```rust
#[derive(Debug)]
pub struct SetMemberAliasRequest {
    pub group_id: ContextGroupId,
    pub member: PublicKey,
    pub alias: String,
    pub requester: Option<PublicKey>,
}

impl Message for SetMemberAliasRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct StoreMemberAliasRequest {
    pub group_id: ContextGroupId,
    pub member: PublicKey,
    pub alias: String,
}

impl Message for StoreMemberAliasRequest {
    type Result = eyre::Result<()>;
}
```

- [ ] **Step 3: cargo check — expect failures in list_group_members handler (missing alias field)**

```bash
cargo check -p calimero-context -p calimero-server 2>&1 | grep "^error" | head -20
```
Expected: errors about missing `alias` field in `GroupMemberEntry { identity, role }` struct literal. These will be fixed in Task 8.

- [ ] **Step 4: Commit**

```bash
git add crates/context/primitives/src/group.rs
git commit -m "feat(context-primitives): add alias to GroupMemberEntry; add SetMemberAlias/StoreMemberAlias requests"
```

---

### Task 5: ContextMessage variants

**Files:**
- Modify: `crates/context/primitives/src/messages.rs`

- [ ] **Step 1: Add new types to the `use crate::group::{...}` import**

Add `SetMemberAliasRequest` and `StoreMemberAliasRequest` to the existing import block (around line 12).

- [ ] **Step 2: Add new ContextMessage variants**

After `StoreContextAllowlist { ... }` (around line 341), add:

```rust
    SetMemberAlias {
        request: SetMemberAliasRequest,
        outcome: oneshot::Sender<<SetMemberAliasRequest as Message>::Result>,
    },
    StoreMemberAlias {
        request: StoreMemberAliasRequest,
        outcome: oneshot::Sender<<StoreMemberAliasRequest as Message>::Result>,
    },
```

- [ ] **Step 3: Commit**

```bash
git add crates/context/primitives/src/messages.rs
git commit -m "feat(context-primitives): add SetMemberAlias/StoreMemberAlias ContextMessage variants"
```

---

### Task 6: ContextClient methods

**Files:**
- Modify: `crates/context/primitives/src/client.rs`

- [ ] **Step 1: Add new types to the `use crate::group::{...}` import block**

Add `SetMemberAliasRequest` and `StoreMemberAliasRequest`.

- [ ] **Step 2: Add two new async methods**

Following the exact pattern of `store_context_alias` and `broadcast_group_aliases`:

```rust
pub async fn set_member_alias(
    &self,
    request: SetMemberAliasRequest,
) -> eyre::Result<()> {
    let (tx, rx) = oneshot::channel();
    self.sender
        .send(ContextMessage::SetMemberAlias {
            request,
            outcome: tx,
        })
        .await?;
    rx.await?
}

pub async fn store_member_alias(
    &self,
    request: StoreMemberAliasRequest,
) -> eyre::Result<()> {
    let (tx, rx) = oneshot::channel();
    self.sender
        .send(ContextMessage::StoreMemberAlias {
            request,
            outcome: tx,
        })
        .await?;
    rx.await?
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/context/primitives/src/client.rs
git commit -m "feat(context-primitives): add set_member_alias/store_member_alias ContextClient methods"
```

---

## Chunk 3: Handler Implementation

### Task 7: New handler files

**Files:**
- Create: `crates/context/src/handlers/set_member_alias.rs`
- Create: `crates/context/src/handlers/store_member_alias.rs`

- [ ] **Step 1: Write `store_member_alias.rs` (sync store handler)**

```rust
use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::StoreMemberAliasRequest;

use crate::{group_store, ContextManager};

impl Handler<StoreMemberAliasRequest> for ContextManager {
    type Result = ActorResponse<Self, <StoreMemberAliasRequest as Message>::Result>;

    fn handle(
        &mut self,
        StoreMemberAliasRequest {
            group_id,
            member,
            alias,
        }: StoreMemberAliasRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result =
            group_store::set_member_alias(&self.datastore, &group_id, &member, &alias);
        ActorResponse::reply(result)
    }
}
```

- [ ] **Step 2: Write `set_member_alias.rs` (write + gossip handler)**

```rust
use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::SetMemberAliasRequest;
use calimero_node_primitives::sync::GroupMutationKind;
use eyre::bail;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<SetMemberAliasRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetMemberAliasRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetMemberAliasRequest {
            group_id,
            member,
            alias,
            requester,
        }: SetMemberAliasRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_group_identity();

        let requester = match requester {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured group identity"
                    )))
                }
            },
        };

        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            if requester != member {
                bail!("members may only set their own alias");
            }

            group_store::set_member_alias(&self.datastore, &group_id, &member, &alias)?;

            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        let node_client = self.node_client.clone();

        ActorResponse::r#async(
            async move {
                info!(
                    ?group_id,
                    %member,
                    %alias,
                    "group member alias set"
                );

                let _ = node_client
                    .broadcast_group_mutation(
                        group_id.to_bytes(),
                        GroupMutationKind::MemberAliasSet {
                            member: *member,
                            alias,
                        },
                    )
                    .await;

                Ok(())
            }
            .into_actor(self),
        )
    }
}
```

- [ ] **Step 3: Run `cargo check -p calimero-context`**

```bash
cargo check -p calimero-context 2>&1 | grep "^error" | head -20
```
Expected: errors about missing module declarations (fixed in Task 8) and the `GroupMemberEntry` struct literal missing `alias` (fixed in Task 8).

- [ ] **Step 4: Commit**

```bash
git add crates/context/src/handlers/set_member_alias.rs
git add crates/context/src/handlers/store_member_alias.rs
git commit -m "feat(context): add set_member_alias and store_member_alias handlers"
```

---

### Task 8: Wire handlers into dispatcher + fix list_group_members

**Files:**
- Modify: `crates/context/src/handlers.rs`
- Modify: `crates/context/src/handlers/list_group_members.rs`

- [ ] **Step 1: Add module declarations to `handlers.rs`**

After `pub mod store_member_capability;` add:
```rust
pub mod set_member_alias;
pub mod store_member_alias;
```

- [ ] **Step 2: Add dispatch arms to `handlers.rs`**

After `ContextMessage::StoreContextAllowlist { request, outcome }` arm (around line 182), add:
```rust
            ContextMessage::SetMemberAlias { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::StoreMemberAlias { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
```

- [ ] **Step 3: Fix `list_group_members.rs` to include alias**

Replace the mapping in `list_group_members.rs`:

```rust
Ok(members
    .into_iter()
    .map(|(identity, role)| GroupMemberEntry { identity, role })
    .collect())
```

With:

```rust
let entries = members
    .into_iter()
    .map(|(identity, role)| {
        let alias =
            group_store::get_member_alias(&self.datastore, &group_id, &identity)
                .ok()
                .flatten();
        GroupMemberEntry {
            identity,
            role,
            alias,
        }
    })
    .collect();
Ok(entries)
```

- [ ] **Step 4: Run `cargo check -p calimero-context`**

```bash
cargo check -p calimero-context 2>&1 | grep "^error" | head -20
```
Expected: no errors (or only errors in calimero-server for the API layer, fixed in Task 9).

- [ ] **Step 5: Commit**

```bash
git add crates/context/src/handlers.rs
git add crates/context/src/handlers/list_group_members.rs
git commit -m "feat(context): wire alias handlers; include alias in list_group_members response"
```

---

### Task 9: Update broadcast_group_local_state.rs

**Files:**
- Modify: `crates/context/src/handlers/broadcast_group_local_state.rs`

- [ ] **Step 1: Read the current file**

Read `crates/context/src/handlers/broadcast_group_local_state.rs` to understand its structure.

- [ ] **Step 2: Add member alias enumeration before the async block**

In the `handle` function, after the existing `allowlists` enumeration and before `let node_client = ...`, add:

```rust
        let member_aliases =
            match group_store::enumerate_member_aliases(&self.datastore, &group_id) {
                Ok(v) => v,
                Err(err) => return ActorResponse::reply(Err(err)),
            };
```

- [ ] **Step 3: Broadcast member aliases inside the async move block**

After the allowlist broadcast loop (around line 98), add:

```rust
                for (member, alias) in member_aliases {
                    let _ = node_client
                        .broadcast_group_mutation(
                            group_id_bytes,
                            GroupMutationKind::MemberAliasSet {
                                member: *member,
                                alias,
                            },
                        )
                        .await;
                }
```

- [ ] **Step 4: Run `cargo check -p calimero-context`**

```bash
cargo check -p calimero-context 2>&1 | grep "^error" | head -20
```
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/context/src/handlers/broadcast_group_local_state.rs
git commit -m "feat(context): re-broadcast member aliases on peer subscription"
```

---

## Chunk 4: Network Event + API Layer

### Task 10: Handle MemberAliasSet in network_event.rs

**Files:**
- Modify: `crates/node/src/handlers/network_event.rs`

- [ ] **Step 1: Read the `GroupMutationNotification` match in `network_event.rs`**

Find the `GroupMutationNotification` match block. Note the `ContextAliasSet` arm as the direct template.

- [ ] **Step 2: Add `MemberAliasSet` arm BEFORE the `_ =>` fallthrough**

Following the exact pattern of the `ContextAliasSet` arm:

```rust
GroupMutationKind::MemberAliasSet { member, alias } => {
    let context_client = self.clients.context.clone();
    let _ignored = ctx.spawn(
        async move {
            use calimero_context_config::types::ContextGroupId;
            use calimero_context_primitives::group::StoreMemberAliasRequest;
            let group_id = ContextGroupId::from(group_id);
            if let Err(err) = context_client
                .store_member_alias(StoreMemberAliasRequest {
                    group_id,
                    member: calimero_primitives::identity::PublicKey::from(member),
                    alias,
                })
                .await
            {
                warn!(?err, "Failed to store member alias from gossip");
            }
        }
        .into_actor(self),
    );
}
```

- [ ] **Step 3: Run `cargo check -p calimero-node`**

```bash
cargo check -p calimero-node 2>&1 | grep "^error" | head -20
```
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/node/src/handlers/network_event.rs
git commit -m "feat(node): handle MemberAliasSet gossip in network_event.rs"
```

---

### Task 11: API types

**Files:**
- Modify: `crates/server/primitives/src/admin.rs`

- [ ] **Step 1: Add `alias` to `GroupMemberApiEntry`**

Change (around line 1937):
```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMemberApiEntry {
    pub identity: PublicKey,
    pub role: GroupMemberRole,
}
```
To:
```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMemberApiEntry {
    pub identity: PublicKey,
    pub role: GroupMemberRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}
```

- [ ] **Step 2: Add `SetMemberAliasApiRequest` and `SetMemberAliasApiResponse`**

After `SetMemberCapabilitiesApiResponse` (around line 2315), add:

```rust
// ---- Set Member Alias ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetMemberAliasApiRequest {
    pub alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for SetMemberAliasApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.alias.is_empty() {
            errors.push(ValidationError::EmptyField { field: "alias" });
        }
        if let Some(e) = validate_string_length(&self.alias, "alias", 64) {
            errors.push(e);
        }
        errors
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetMemberAliasApiResponse;
```

Note: `validate_string_length` and `ValidationError` are already defined in `admin.rs` — no new imports needed.

- [ ] **Step 3: Run `cargo check -p calimero-server-primitives`**

```bash
cargo check -p calimero-server-primitives 2>&1 | grep "^error" | head -20
```
Expected: no errors.

- [ ] **Step 4: Fix list_group_members server handler to pass alias through**

In `crates/server/src/admin/handlers/groups/list_group_members.rs`, the mapping:
```rust
.map(|m| GroupMemberApiEntry {
    identity: m.identity,
    role: m.role,
})
```
becomes:
```rust
.map(|m| GroupMemberApiEntry {
    identity: m.identity,
    role: m.role,
    alias: m.alias,
})
```

- [ ] **Step 5: Commit**

```bash
git add crates/server/primitives/src/admin.rs
git add crates/server/src/admin/handlers/groups/list_group_members.rs
git commit -m "feat(server-primitives): add alias to GroupMemberApiEntry; add SetMemberAlias API types"
```

---

### Task 12: New REST handler + route

**Files:**
- Create: `crates/server/src/admin/handlers/groups/set_member_alias.rs`
- Modify: `crates/server/src/admin/handlers/groups.rs`
- Modify: `crates/server/src/admin/service.rs`

- [ ] **Step 1: Write `set_member_alias.rs`**

Following the exact pattern of `set_member_capabilities.rs`:

```rust
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::SetMemberAliasRequest;
use calimero_server_primitives::admin::SetMemberAliasApiRequest;
use tracing::{error, info};

use super::{parse_group_id, parse_identity};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse, Empty};
use crate::AdminState;

pub async fn handler(
    Path((group_id_str, identity_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<SetMemberAliasApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let member = match parse_identity(&identity_str) {
        Ok(pk) => pk,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, identity=%identity_str, alias=%req.alias, "Setting member alias");

    let result = state
        .ctx_client
        .set_member_alias(SetMemberAliasRequest {
            group_id,
            member,
            alias: req.alias,
            requester: req.requester,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, identity=%identity_str, "Member alias set");
            ApiResponse { payload: Empty }.into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, identity=%identity_str, error=?err, "Failed to set member alias");
            err.into_response()
        }
    }
}
```

- [ ] **Step 2: Add module declaration to `groups.rs`**

Add `pub mod set_member_alias;` in alphabetical order (after `set_default_visibility`, before `set_member_capabilities`).

- [ ] **Step 3: Add route to `service.rs`**

After the `/groups/:group_id/members/:identity/role` route (around line 250), add:

```rust
        .route(
            "/groups/:group_id/members/:identity/alias",
            put(groups::set_member_alias::handler),
        )
```

- [ ] **Step 4: Run full cargo check**

```bash
cargo check -p calimero-context -p calimero-node -p calimero-server 2>&1 | grep "^error" | head -20
```
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/admin/handlers/groups/set_member_alias.rs
git add crates/server/src/admin/handlers/groups.rs
git add crates/server/src/admin/service.rs
git commit -m "feat(server): add PUT /groups/:group_id/members/:identity/alias endpoint"
```

---

## Chunk 5: Verification

### Task 13: Full build and test pass

- [ ] **Step 1: Format check**

```bash
cargo fmt --check -p calimero-store -p calimero-context -p calimero-node -p calimero-server
```
If it fails, run `cargo fmt` then re-check.

- [ ] **Step 2: Clippy**

```bash
cargo clippy -p calimero-store -p calimero-context -p calimero-node -p calimero-server -- -A warnings 2>&1 | grep "^error" | head -20
```
Expected: no errors.

- [ ] **Step 3: Test suite**

```bash
cargo test -p calimero-store -p calimero-context -p calimero-node -p calimero-server 2>&1 | tail -30
```
Expected: all tests pass.

- [ ] **Step 4: Workspace check (ensures no cross-crate regressions)**

```bash
cargo check --workspace 2>&1 | grep "^error" | head -20
```
Expected: no errors.

- [ ] **Step 5: Final commit if needed**

```bash
git add -p  # review any unstaged formatting changes
git commit -m "style: apply cargo fmt to member alias implementation"
```

---

## Manual Smoke Test

1. Start two nodes (Node-A and Node-B in the same group)
2. On Node-A: `PUT /admin-api/groups/:group_id/members/:identity/alias` with `{ "alias": "alice" }`
3. On Node-A: `GET /admin-api/groups/:group_id/members` — verify `{ "identity": "...", "role": "member", "alias": "alice" }`
4. On Node-B: `GET /admin-api/groups/:group_id/members` — verify `alias` is `"alice"` (received via gossip)
5. Start Node-C fresh, subscribe to the group — verify aliases are re-broadcast via `broadcast_group_local_state`
6. Attempt: `PUT .../members/:identity/alias` where requester ≠ member — expect `400 Bad Request`

---

## Key Implementation Notes

- **Borsh discriminant safety**: `MemberAliasSet` is appended after `ContextAllowlistSet` (discriminant 14) → gets discriminant 15. Never reorder existing variants.
- **Value type**: Member alias stored as bare `String` (Borsh), matching `GroupContextAlias` precedent — no wrapper struct needed.
- **Authorization**: Self-only (`requester == member`). No chain signing key required (alias is never written to chain).
- **Re-broadcast**: `broadcast_group_local_state` handles late-joining peers — same path as context aliases + other local state.
- **`distinct_prefixes` test**: Must add `GROUP_MEMBER_ALIAS_PREFIX` to the test array in `store/src/key/group.rs` or the test will fail with a collision assertion.
- **`alias` on `GroupMemberEntry`**: Populated by calling `group_store::get_member_alias` for each member in `list_group_members.rs` — O(n) lookups, acceptable for typical group sizes.
- **`skip_serializing_if = "Option::is_none"`**: Keeps the REST response backward-compatible for clients that don't know about aliases yet.
