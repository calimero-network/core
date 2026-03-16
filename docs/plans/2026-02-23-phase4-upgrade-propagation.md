# Phase 4: Upgrade Propagation — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Single admin trigger propagates an application upgrade across all contexts in a group, with canary testing, progress tracking, and retry support.

**Architecture:** An admin calls `POST /groups/:id/upgrade` which validates permissions, runs a canary upgrade on the first context, then spawns a background `UpgradePropagator` future on the actix actor event loop. The propagator iterates remaining contexts, calling `context_client.update_application()` for each (leveraging actix's cooperative scheduling — the spawned future yields at each `.await`, allowing the actor to process the `UpdateApplication` messages in between). Progress is persisted to the store after each context via `GroupUpgradeValue`. Two query/retry endpoints let admins check status and retry failed contexts.

**Tech Stack:** Rust, actix (actor framework), calimero-store (RocksDB), eyre (errors), tokio (async)

**Branch:** `feat/context-management-proposal` (continue existing work)

---

## Prerequisites

Before starting, verify:
```bash
cargo check --workspace   # Must pass
cargo fmt --check          # Must pass
cargo clippy -- -A warnings  # Must pass
```

Phases 1-3 must be complete and compiling. The uncommitted Phase 3 changes should be committed first.

---

## Key Files Reference

| File | Role |
|------|------|
| `crates/context/primitives/src/group.rs` | Message types for upgrade requests |
| `crates/context/primitives/src/messages.rs` | ContextMessage enum variants |
| `crates/context/primitives/src/client.rs` | ContextClient methods |
| `crates/context/src/handlers/upgrade_group.rs` | **New** — main upgrade handler |
| `crates/context/src/handlers/get_group_upgrade_status.rs` | **New** — status query handler |
| `crates/context/src/handlers/retry_group_upgrade.rs` | **New** — retry handler |
| `crates/context/src/handlers.rs` | Module declarations + match arms |
| `crates/context/src/group_store.rs` | Existing store helpers (already complete) |
| `crates/context/src/handlers/update_application.rs` | Existing — `update_application_id()` fn (public) |
| `crates/store/src/key/group.rs` | GroupUpgradeValue, GroupUpgradeStatus types (already complete) |
| `crates/server/src/admin/handlers/groups/` | Server handlers |
| `crates/server/src/admin/service.rs` | Route registration |
| `crates/server/primitives/src/admin.rs` | API request/response types |

---

## Critical Design Decisions

### 1. Canary Pattern
The first context in the group (deterministically sorted by `enumerate_group_contexts`) is upgraded first. If it fails, the entire upgrade is aborted with no state changes. If it succeeds, the propagator handles the rest.

### 2. Actix Cooperative Scheduling
The `UpgradePropagator` is spawned via `actix::Context::spawn()` on the actor's event loop. It calls `context_client.update_application()` which sends `ContextMessage::UpdateApplication` back to the actor's mailbox. Since the spawned future `.await`s each call, the actor processes the message between polls. This is safe — no deadlock, no re-entrancy issues.

### 3. No New Migration Logic
The propagator calls existing `context_client.update_application()` which routes to the existing `Handler<UpdateApplicationRequest>`. All migration logic is reused as-is.

### 4. Progress Persistence
`GroupUpgradeValue` is saved to the store after each context upgrade (success or failure). This enables crash recovery (Phase 5) and status queries.

### 5. AppKey Continuity
The existing `verify_appkey_continuity()` in `update_application.rs` already validates that upgrades preserve the signer identity. The upgrade handler additionally validates that the new application_id shares the same AppKey as the group.

---

## Task 1: Add Message Types for Upgrade Operations

**Files:**
- Modify: `crates/context/primitives/src/group.rs` (append after ListGroupContextsRequest)
- Modify: `crates/context/primitives/src/messages.rs` (add 3 ContextMessage variants)

### Step 1: Add request/response types to group.rs

Append after the existing `ListGroupContextsRequest` block:

```rust
#[derive(Debug, Clone)]
pub struct UpgradeGroupRequest {
    pub group_id: ContextGroupId,
    pub target_application_id: ApplicationId,
    pub requester: PublicKey,
    pub migration: Option<MigrationParams>,
}

impl Message for UpgradeGroupRequest {
    type Result = eyre::Result<UpgradeGroupResponse>;
}

#[derive(Clone, Debug)]
pub struct UpgradeGroupResponse {
    pub group_id: ContextGroupId,
    pub status: GroupUpgradeStatus,
}

#[derive(Debug)]
pub struct GetGroupUpgradeStatusRequest {
    pub group_id: ContextGroupId,
}

impl Message for GetGroupUpgradeStatusRequest {
    type Result = eyre::Result<Option<GroupUpgradeValue>>;
}

#[derive(Debug)]
pub struct RetryGroupUpgradeRequest {
    pub group_id: ContextGroupId,
    pub requester: PublicKey,
}

impl Message for RetryGroupUpgradeRequest {
    type Result = eyre::Result<UpgradeGroupResponse>;
}
```

**New imports needed at top of group.rs:**
```rust
use calimero_primitives::application::ApplicationId;
use crate::messages::MigrationParams;
use calimero_store::key::GroupUpgradeStatus;  // already have GroupUpgradeValue via get_group_info
```

Wait — check existing imports first. `ApplicationId` is already imported. `GroupUpgradeValue` is already used in `GroupInfoResponse`. Add `GroupUpgradeStatus` and `MigrationParams`.

### Step 2: Add ContextMessage variants to messages.rs

Add these imports to the `use crate::group::` block:
```rust
use crate::group::{
    // ... existing imports ...,
    GetGroupUpgradeStatusRequest, RetryGroupUpgradeRequest, UpgradeGroupRequest,
};
```

Add variants before the closing `}` of `ContextMessage`:
```rust
    UpgradeGroup {
        request: UpgradeGroupRequest,
        outcome: oneshot::Sender<<UpgradeGroupRequest as Message>::Result>,
    },
    GetGroupUpgradeStatus {
        request: GetGroupUpgradeStatusRequest,
        outcome: oneshot::Sender<<GetGroupUpgradeStatusRequest as Message>::Result>,
    },
    RetryGroupUpgrade {
        request: RetryGroupUpgradeRequest,
        outcome: oneshot::Sender<<RetryGroupUpgradeRequest as Message>::Result>,
    },
```

### Step 3: Verify compilation

```bash
cargo check -p calimero-context-primitives
```
Expected: Success (may have pre-existing warnings)

### Step 4: Commit

```bash
git add crates/context/primitives/src/group.rs crates/context/primitives/src/messages.rs
git commit -m "feat(group): add upgrade message types (UpgradeGroup, GetGroupUpgradeStatus, RetryGroupUpgrade)"
```

---

## Task 2: Add ContextClient Methods for Upgrade Operations

**Files:**
- Modify: `crates/context/primitives/src/client.rs` (add 3 methods)

### Step 1: Add imports

Add to the existing `use crate::group::` block:
```rust
use crate::group::{
    // ... existing ...,
    GetGroupUpgradeStatusRequest, RetryGroupUpgradeRequest, UpgradeGroupRequest,
    UpgradeGroupResponse,
};
```

### Step 2: Add methods to ContextClient

Append after `list_group_contexts()`:

```rust
    pub async fn upgrade_group(
        &self,
        request: UpgradeGroupRequest,
    ) -> eyre::Result<UpgradeGroupResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::UpgradeGroup {
                request,
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    pub async fn get_group_upgrade_status(
        &self,
        request: GetGroupUpgradeStatusRequest,
    ) -> eyre::Result<Option<GroupUpgradeValue>> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::GetGroupUpgradeStatus {
                request,
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    pub async fn retry_group_upgrade(
        &self,
        request: RetryGroupUpgradeRequest,
    ) -> eyre::Result<UpgradeGroupResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::RetryGroupUpgrade {
                request,
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }
```

**New import for GroupUpgradeValue:**
```rust
use calimero_store::key::GroupUpgradeValue;
```

### Step 3: Verify compilation

```bash
cargo check -p calimero-context-primitives
```

### Step 4: Commit

```bash
git add crates/context/primitives/src/client.rs
git commit -m "feat(group): add ContextClient methods for upgrade operations"
```

---

## Task 3: Implement UpgradeGroup Handler (Canary + Propagator Spawn)

This is the most complex task. The handler:
1. Validates admin, no active upgrade
2. Runs canary upgrade on the first context
3. On success: updates group target, persists InProgress, spawns propagator
4. Returns InProgress status

**Files:**
- Create: `crates/context/src/handlers/upgrade_group.rs`

### Step 1: Create the handler file

```rust
use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorFutureExt, ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::group::{UpgradeGroupRequest, UpgradeGroupResponse};
use calimero_context_primitives::messages::MigrationParams;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};
use eyre::bail;
use tracing::{debug, error, info, warn};

use crate::{group_store, ContextManager};

impl Handler<UpgradeGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <UpgradeGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        UpgradeGroupRequest {
            group_id,
            target_application_id,
            requester,
            migration,
        }: UpgradeGroupRequest,
        ctx: &mut Self::Context,
    ) -> Self::Result {
        // --- Synchronous validation ---
        let preamble = match validate_upgrade(
            &self.datastore,
            &group_id,
            &target_application_id,
            &requester,
        ) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let UpgradePreamble {
            canary_context_id,
            total_contexts,
        } = preamble;

        // --- Async: run canary upgrade ---
        let context_client = self.context_client.clone();
        let datastore = self.datastore.clone();
        let migrate_method = migration.as_ref().map(|m| m.method.clone());

        let canary_task = async move {
            context_client
                .update_application(
                    &canary_context_id,
                    &target_application_id,
                    &requester,
                    migrate_method,
                )
                .await
        }
        .into_actor(self);

        let group_id_clone = group_id;
        let context_client_for_propagator = self.context_client.clone();
        let datastore_for_propagator = self.datastore.clone();

        ActorResponse::r#async(
            canary_task
                .map(move |canary_result, _act, ctx| {
                    match canary_result {
                        Err(err) => {
                            error!(
                                ?group_id,
                                canary=%canary_context_id,
                                ?err,
                                "canary upgrade failed, aborting group upgrade"
                            );
                            Err(eyre::eyre!(
                                "canary upgrade failed on context {canary_context_id}: {err}"
                            ))
                        }
                        Ok(()) => {
                            info!(
                                ?group_id,
                                canary=%canary_context_id,
                                "canary upgrade succeeded, proceeding with group upgrade"
                            );

                            // Update group's target_application_id
                            let mut meta = group_store::load_group_meta(
                                &datastore,
                                &group_id_clone,
                            )?
                            .ok_or_else(|| eyre::eyre!("group not found after canary"))?;

                            meta.target_application_id = target_application_id;
                            group_store::save_group_meta(
                                &datastore,
                                &group_id_clone,
                                &meta,
                            )?;

                            // Persist InProgress status (canary = 1 completed)
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();

                            let status = GroupUpgradeStatus::InProgress {
                                total: total_contexts as u32,
                                completed: 1,
                                failed: 0,
                            };

                            let upgrade_value = GroupUpgradeValue {
                                from_revision: 0,
                                to_revision: 0,
                                migration: migration
                                    .as_ref()
                                    .map(|m| m.method.as_bytes().to_vec()),
                                initiated_at: now,
                                initiated_by: *requester,
                                status: status.clone(),
                            };

                            group_store::save_group_upgrade(
                                &datastore,
                                &group_id_clone,
                                &upgrade_value,
                            )?;

                            // Spawn propagator for remaining contexts
                            if total_contexts > 1 {
                                let propagator = propagate_upgrade(
                                    context_client_for_propagator,
                                    datastore_for_propagator,
                                    group_id_clone,
                                    target_application_id,
                                    requester,
                                    migration,
                                    canary_context_id,
                                    total_contexts,
                                );
                                ctx.spawn(propagator.into_actor(_act));
                            } else {
                                // Only one context (the canary) — mark completed
                                let completed_status = GroupUpgradeStatus::Completed {
                                    completed_at: now,
                                };
                                let mut completed_value = upgrade_value;
                                completed_value.status = completed_status.clone();
                                group_store::save_group_upgrade(
                                    &datastore,
                                    &group_id_clone,
                                    &completed_value,
                                )?;

                                return Ok(UpgradeGroupResponse {
                                    group_id: group_id_clone,
                                    status: completed_status,
                                });
                            }

                            Ok(UpgradeGroupResponse {
                                group_id: group_id_clone,
                                status,
                            })
                        }
                    }
                }),
        )
    }
}

struct UpgradePreamble {
    canary_context_id: ContextId,
    total_contexts: usize,
}

fn validate_upgrade(
    datastore: &calimero_store::Store,
    group_id: &ContextGroupId,
    target_application_id: &ApplicationId,
    requester: &PublicKey,
) -> eyre::Result<UpgradePreamble> {
    // 1. Group must exist
    let meta = group_store::load_group_meta(datastore, group_id)?
        .ok_or_else(|| eyre::eyre!("group not found"))?;

    // 2. Requester must be admin
    group_store::require_group_admin(datastore, group_id, requester)?;

    // 3. No active upgrade in progress
    if let Some(existing) = group_store::load_group_upgrade(datastore, group_id)? {
        if matches!(existing.status, GroupUpgradeStatus::InProgress { .. }) {
            bail!("an upgrade is already in progress for this group");
        }
    }

    // 4. Target must differ from current
    if meta.target_application_id == *target_application_id {
        bail!("group is already targeting this application");
    }

    // 5. Group must have contexts
    let contexts = group_store::enumerate_group_contexts(datastore, group_id)?;
    if contexts.is_empty() {
        bail!("group has no contexts to upgrade");
    }

    // 6. Select canary (first context, deterministic order)
    let canary_context_id = contexts[0];

    Ok(UpgradePreamble {
        canary_context_id,
        total_contexts: contexts.len(),
    })
}

async fn propagate_upgrade(
    context_client: calimero_context_primitives::client::ContextClient,
    datastore: calimero_store::Store,
    group_id: ContextGroupId,
    target_application_id: ApplicationId,
    requester: PublicKey,
    migration: Option<MigrationParams>,
    skip_context: ContextId,
    total_contexts: usize,
) {
    let contexts = match group_store::enumerate_group_contexts(&datastore, &group_id) {
        Ok(c) => c,
        Err(err) => {
            error!(?group_id, ?err, "failed to enumerate contexts for propagation");
            return;
        }
    };

    let mut completed: u32 = 1; // canary already done
    let mut failed: u32 = 0;

    for context_id in contexts {
        if context_id == skip_context {
            continue;
        }

        let migrate_method = migration.as_ref().map(|m| m.method.clone());

        match context_client
            .update_application(&context_id, &target_application_id, &requester, migrate_method)
            .await
        {
            Ok(()) => {
                completed += 1;
                debug!(
                    ?group_id,
                    %context_id,
                    completed,
                    total = total_contexts,
                    "context upgraded successfully"
                );
            }
            Err(err) => {
                failed += 1;
                warn!(
                    ?group_id,
                    %context_id,
                    ?err,
                    failed,
                    "context upgrade failed"
                );
            }
        }

        // Persist progress after each context
        let status = GroupUpgradeStatus::InProgress {
            total: total_contexts as u32,
            completed,
            failed,
        };

        if let Err(err) = update_upgrade_status(&datastore, &group_id, status) {
            error!(?group_id, ?err, "failed to persist upgrade progress");
        }
    }

    // Final status
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let final_status = if failed == 0 {
        GroupUpgradeStatus::Completed {
            completed_at: now,
        }
    } else {
        // Keep as InProgress with the final counts so retry can pick it up
        GroupUpgradeStatus::InProgress {
            total: total_contexts as u32,
            completed,
            failed,
        }
    };

    if let Err(err) = update_upgrade_status(&datastore, &group_id, final_status.clone()) {
        error!(?group_id, ?err, "failed to persist final upgrade status");
    }

    info!(
        ?group_id,
        completed,
        failed,
        total = total_contexts,
        "group upgrade propagation finished"
    );
}

fn update_upgrade_status(
    datastore: &calimero_store::Store,
    group_id: &ContextGroupId,
    status: GroupUpgradeStatus,
) -> eyre::Result<()> {
    if let Some(mut upgrade) = group_store::load_group_upgrade(datastore, group_id)? {
        upgrade.status = status;
        group_store::save_group_upgrade(datastore, group_id, &upgrade)?;
    }
    Ok(())
}
```

### Step 2: Verify compilation

```bash
cargo check -p calimero-context
```

### Step 3: Commit

```bash
git add crates/context/src/handlers/upgrade_group.rs
git commit -m "feat(group): implement UpgradeGroup handler with canary testing and background propagator"
```

---

## Task 4: Implement GetGroupUpgradeStatus Handler

**Files:**
- Create: `crates/context/src/handlers/get_group_upgrade_status.rs`

### Step 1: Create the handler

```rust
use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::GetGroupUpgradeStatusRequest;
use calimero_store::key::GroupUpgradeValue;

use crate::{group_store, ContextManager};

impl Handler<GetGroupUpgradeStatusRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetGroupUpgradeStatusRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetGroupUpgradeStatusRequest { group_id }: GetGroupUpgradeStatusRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = group_store::load_group_upgrade(&self.datastore, &group_id);
        ActorResponse::reply(result)
    }
}
```

### Step 2: Verify compilation

```bash
cargo check -p calimero-context
```

### Step 3: Commit

```bash
git add crates/context/src/handlers/get_group_upgrade_status.rs
git commit -m "feat(group): implement GetGroupUpgradeStatus handler"
```

---

## Task 5: Implement RetryGroupUpgrade Handler

**Files:**
- Create: `crates/context/src/handlers/retry_group_upgrade.rs`

### Step 1: Create the handler

```rust
use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::{RetryGroupUpgradeRequest, UpgradeGroupResponse};
use calimero_context_primitives::messages::MigrationParams;
use calimero_store::key::GroupUpgradeStatus;
use eyre::bail;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<RetryGroupUpgradeRequest> for ContextManager {
    type Result = ActorResponse<Self, <RetryGroupUpgradeRequest as Message>::Result>;

    fn handle(
        &mut self,
        RetryGroupUpgradeRequest {
            group_id,
            requester,
        }: RetryGroupUpgradeRequest,
        ctx: &mut Self::Context,
    ) -> Self::Result {
        // Validate
        let result = (|| {
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            let upgrade = group_store::load_group_upgrade(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("no upgrade found for this group"))?;

            let (total, _completed, failed) = match upgrade.status {
                GroupUpgradeStatus::InProgress {
                    total,
                    completed,
                    failed,
                } if failed > 0 => (total, completed, failed),
                GroupUpgradeStatus::InProgress { failed: 0, .. } => {
                    bail!("upgrade is in progress with no failures — nothing to retry");
                }
                GroupUpgradeStatus::Completed { .. } => {
                    bail!("upgrade is already completed");
                }
                GroupUpgradeStatus::RolledBack { .. } => {
                    bail!("upgrade has been rolled back — start a new upgrade instead");
                }
                _ => bail!("unexpected upgrade status"),
            };

            let meta = group_store::load_group_meta(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            let migration = upgrade
                .migration
                .as_ref()
                .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
                .map(|method| MigrationParams { method });

            Ok((meta.target_application_id, migration, total))
        })();

        let (target_application_id, migration, total) = match result {
            Ok(v) => v,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        info!(
            ?group_id,
            %requester,
            "retrying group upgrade for failed contexts"
        );

        // Re-spawn propagator (it skips contexts already at target version)
        let context_client = self.context_client.clone();
        let datastore = self.datastore.clone();

        let propagator = super::upgrade_group::propagate_upgrade(
            context_client,
            datastore,
            group_id,
            target_application_id,
            requester,
            migration,
            // No canary skip on retry — the propagator checks each context's
            // current application_id vs target and skips already-upgraded ones
            calimero_primitives::context::ContextId::from([0u8; 32]),
            total as usize,
        );

        ctx.spawn(propagator.into_actor(self));

        let status = GroupUpgradeStatus::InProgress {
            total,
            completed: 0,
            failed: 0,
        };

        ActorResponse::reply(Ok(UpgradeGroupResponse { group_id, status }))
    }
}
```

**Note:** This requires `propagate_upgrade` in `upgrade_group.rs` to be `pub(super)` or `pub(crate)`.

### Step 2: Make propagate_upgrade visible

In `crates/context/src/handlers/upgrade_group.rs`, change:
```rust
async fn propagate_upgrade(
```
to:
```rust
pub(crate) async fn propagate_upgrade(
```

### Step 3: Verify compilation

```bash
cargo check -p calimero-context
```

### Step 4: Commit

```bash
git add crates/context/src/handlers/retry_group_upgrade.rs crates/context/src/handlers/upgrade_group.rs
git commit -m "feat(group): implement RetryGroupUpgrade handler"
```

---

## Task 6: Wire Handlers into ContextManager

**Files:**
- Modify: `crates/context/src/handlers.rs` (add modules + match arms)

### Step 1: Add module declarations

After `pub mod list_group_members;`:
```rust
pub mod get_group_upgrade_status;
pub mod retry_group_upgrade;
pub mod upgrade_group;
```

### Step 2: Add match arms

In the `Handler<ContextMessage>` match block, before the closing `}`:
```rust
            ContextMessage::UpgradeGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::GetGroupUpgradeStatus { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::RetryGroupUpgrade { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
```

### Step 3: Verify compilation

```bash
cargo check -p calimero-context
```

### Step 4: Commit

```bash
git add crates/context/src/handlers.rs
git commit -m "feat(group): wire upgrade handlers into ContextManager message dispatch"
```

---

## Task 7: Add Server API Types

**Files:**
- Modify: `crates/server/primitives/src/admin.rs` (add request/response types)

### Step 1: Add types after ListGroupContextsQuery

```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeGroupApiRequest {
    pub target_application_id: ApplicationId,
    pub requester: PublicKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migrate_method: Option<String>,
}

impl Validate for UpgradeGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeGroupApiResponse {
    pub data: UpgradeGroupApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeGroupApiResponseData {
    pub group_id: String,
    pub status: String,
    pub total: Option<u32>,
    pub completed: Option<u32>,
    pub failed: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetGroupUpgradeStatusApiResponse {
    pub data: Option<GroupUpgradeStatusApiData>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupUpgradeStatusApiData {
    pub from_revision: u64,
    pub to_revision: u64,
    pub initiated_at: u64,
    pub initiated_by: PublicKey,
    pub status: String,
    pub total: Option<u32>,
    pub completed: Option<u32>,
    pub failed: Option<u32>,
    pub completed_at: Option<u64>,
    pub rollback_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryGroupUpgradeApiRequest {
    pub requester: PublicKey,
}

impl Validate for RetryGroupUpgradeApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}
```

### Step 2: Verify compilation

```bash
cargo check -p calimero-server-primitives
```

### Step 3: Commit

```bash
git add crates/server/primitives/src/admin.rs
git commit -m "feat(group): add server API types for upgrade endpoints"
```

---

## Task 8: Create Server Handlers for Upgrade Endpoints

**Files:**
- Create: `crates/server/src/admin/handlers/groups/upgrade_group.rs`
- Create: `crates/server/src/admin/handlers/groups/get_group_upgrade_status.rs`
- Create: `crates/server/src/admin/handlers/groups/retry_group_upgrade.rs`

### Step 1: Create upgrade_group.rs

```rust
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_context_primitives::group::UpgradeGroupRequest;
use calimero_context_primitives::messages::MigrationParams;
use calimero_server_primitives::admin::{
    UpgradeGroupApiRequest, UpgradeGroupApiResponse, UpgradeGroupApiResponseData,
};
use calimero_store::key::GroupUpgradeStatus;
use reqwest::StatusCode;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<UpgradeGroupApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, %req.target_application_id, "Initiating group upgrade");

    let migration = req
        .migrate_method
        .map(|method| MigrationParams { method });

    let result = state
        .ctx_client
        .upgrade_group(UpgradeGroupRequest {
            group_id,
            target_application_id: req.target_application_id,
            requester: req.requester,
            migration,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            let (status_str, total, completed, failed) = format_status(&resp.status);
            info!(group_id=%group_id_str, %status_str, "Group upgrade initiated");
            ApiResponse {
                payload: UpgradeGroupApiResponse {
                    data: UpgradeGroupApiResponseData {
                        group_id: hex::encode(resp.group_id.to_bytes()),
                        status: status_str,
                        total,
                        completed,
                        failed,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to initiate group upgrade");
            err.into_response()
        }
    }
}

pub fn format_status(
    status: &GroupUpgradeStatus,
) -> (String, Option<u32>, Option<u32>, Option<u32>) {
    match status {
        GroupUpgradeStatus::InProgress {
            total,
            completed,
            failed,
        } => (
            "in_progress".to_owned(),
            Some(*total),
            Some(*completed),
            Some(*failed),
        ),
        GroupUpgradeStatus::Completed { .. } => {
            ("completed".to_owned(), None, None, None)
        }
        GroupUpgradeStatus::RolledBack { .. } => {
            ("rolled_back".to_owned(), None, None, None)
        }
    }
}
```

### Step 2: Create get_group_upgrade_status.rs

```rust
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::GetGroupUpgradeStatusRequest;
use calimero_server_primitives::admin::{
    GetGroupUpgradeStatusApiResponse, GroupUpgradeStatusApiData,
};
use calimero_store::key::GroupUpgradeStatus;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Getting group upgrade status");

    let result = state
        .ctx_client
        .get_group_upgrade_status(GetGroupUpgradeStatusRequest { group_id })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(upgrade) => {
            let data = upgrade.map(|u| {
                let (status, total, completed, failed, completed_at, rollback_reason) =
                    match &u.status {
                        GroupUpgradeStatus::InProgress {
                            total,
                            completed,
                            failed,
                        } => (
                            "in_progress",
                            Some(*total),
                            Some(*completed),
                            Some(*failed),
                            None,
                            None,
                        ),
                        GroupUpgradeStatus::Completed { completed_at } => {
                            ("completed", None, None, None, Some(*completed_at), None)
                        }
                        GroupUpgradeStatus::RolledBack { reason } => (
                            "rolled_back",
                            None,
                            None,
                            None,
                            None,
                            Some(reason.clone()),
                        ),
                    };

                GroupUpgradeStatusApiData {
                    from_revision: u.from_revision,
                    to_revision: u.to_revision,
                    initiated_at: u.initiated_at,
                    initiated_by: u.initiated_by,
                    status: status.to_owned(),
                    total,
                    completed,
                    failed,
                    completed_at,
                    rollback_reason,
                }
            });

            ApiResponse {
                payload: GetGroupUpgradeStatusApiResponse { data },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to get upgrade status");
            err.into_response()
        }
    }
}
```

### Step 3: Create retry_group_upgrade.rs

```rust
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_context_primitives::group::RetryGroupUpgradeRequest;
use calimero_server_primitives::admin::{
    RetryGroupUpgradeApiRequest, UpgradeGroupApiResponse, UpgradeGroupApiResponseData,
};
use tracing::{error, info};

use super::parse_group_id;
use super::upgrade_group::format_status;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<RetryGroupUpgradeApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Retrying group upgrade");

    let result = state
        .ctx_client
        .retry_group_upgrade(RetryGroupUpgradeRequest {
            group_id,
            requester: req.requester,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            let (status_str, total, completed, failed) = format_status(&resp.status);
            info!(group_id=%group_id_str, "Group upgrade retry initiated");
            ApiResponse {
                payload: UpgradeGroupApiResponse {
                    data: UpgradeGroupApiResponseData {
                        group_id: hex::encode(resp.group_id.to_bytes()),
                        status: status_str,
                        total,
                        completed,
                        failed,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to retry group upgrade");
            err.into_response()
        }
    }
}
```

### Step 4: Add modules to groups.rs

Add to `crates/server/src/admin/handlers/groups.rs`:
```rust
pub mod get_group_upgrade_status;
pub mod retry_group_upgrade;
pub mod upgrade_group;
```

### Step 5: Add routes to service.rs

In `crates/server/src/admin/service.rs`, after the `/groups/:group_id/members/remove` route:
```rust
        .route(
            "/groups/:group_id/upgrade",
            post(groups::upgrade_group::handler),
        )
        .route(
            "/groups/:group_id/upgrade/status",
            get(groups::get_group_upgrade_status::handler),
        )
        .route(
            "/groups/:group_id/upgrade/retry",
            post(groups::retry_group_upgrade::handler),
        )
```

### Step 6: Verify compilation

```bash
cargo check --workspace
cargo fmt --check
cargo clippy -- -A warnings
```

### Step 7: Commit

```bash
git add crates/server/src/admin/handlers/groups/ crates/server/src/admin/handlers/groups.rs crates/server/src/admin/service.rs
git commit -m "feat(group): add server endpoints for group upgrade (POST upgrade, GET status, POST retry)"
```

---

## Task 9: Update Implementation Plan Doc

**Files:**
- Modify: `CONTEXT_GROUPS_IMPL_PLAN.md`

### Step 1: Mark Phase 4 tasks completed

Change status from `pending` to `completed` for tasks `p4-msg-upgrade-variants` through `p4-ctx-doc-update`.

### Step 2: Update file change table

Add Phase 4 entries:
```
| P4    | `context/primitives/src/group.rs`                     | Modify  | +40      | ✅      |
| P4    | `context/primitives/src/messages.rs`                   | Modify  | +15      | ✅      |
| P4    | `context/primitives/src/client.rs`                     | Modify  | +40      | ✅      |
| P4    | `context/src/handlers/upgrade_group.rs`                | **New** | ~250     | ✅      |
| P4    | `context/src/handlers/get_group_upgrade_status.rs`    | **New** | ~15      | ✅      |
| P4    | `context/src/handlers/retry_group_upgrade.rs`          | **New** | ~80      | ✅      |
| P4    | `context/src/handlers.rs`                              | Modify  | +15      | ✅      |
| P4    | `server/primitives/src/admin.rs`                       | Modify  | +60      | ✅      |
| P4    | `server/src/admin/handlers/groups/upgrade_group.rs`   | **New** | ~80      | ✅      |
| P4    | `server/src/admin/handlers/groups/get_group_upgrade_status.rs`| **New** | ~80 | ✅  |
| P4    | `server/src/admin/handlers/groups/retry_group_upgrade.rs`| **New** | ~50     | ✅      |
| P4    | `server/src/admin/handlers/groups.rs`                  | Modify  | +3       | ✅      |
| P4    | `server/src/admin/service.rs`                          | Modify  | +12      | ✅      |
```

### Step 3: Commit

```bash
git add CONTEXT_GROUPS_IMPL_PLAN.md
git commit -m "docs(group): mark Phase 4 completed in implementation plan"
```

---

## New API Routes (Phase 4)

```
POST   /admin-api/groups/:group_id/upgrade           → UpgradeGroup (returns InProgress/Completed)
GET    /admin-api/groups/:group_id/upgrade/status     → GetGroupUpgradeStatus
POST   /admin-api/groups/:group_id/upgrade/retry      → RetryGroupUpgrade
```

---

## Verification Checklist

After all tasks:
- [ ] `cargo check --workspace` passes
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -A warnings` passes
- [ ] All 3 new API routes are registered in service.rs
- [ ] All 3 new ContextMessage variants have match arms in handlers.rs
- [ ] GroupUpgradeValue status is persisted after each context upgrade
- [ ] Canary failure aborts the upgrade with no state changes
- [ ] Retry handler validates failed > 0 before proceeding
- [ ] No dead code, no unused imports

---

## Execution Batches

| Batch | Tasks | Focus |
|-------|-------|-------|
| 1 | 1, 2 | Message types + ContextClient methods |
| 2 | 3, 4, 5, 6 | Core handlers (upgrade, status, retry) + wiring |
| 3 | 7, 8 | Server API types + endpoint handlers |
| 4 | 9 | Documentation update |
