# Phase 5 — Advanced Policies + Crash Recovery Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add lazy-on-access transparent upgrade during method execution, startup crash recovery for in-progress upgrades, and a rollback endpoint that reverses a group upgrade.

**Architecture:** Three independent features: (1) a `maybe_lazy_upgrade()` check inserted into the execute handler that transparently upgrades stale contexts before method execution, (2) a `recover_in_progress_upgrades()` method on ContextManager that scans for incomplete upgrades at startup and re-spawns propagators, (3) a rollback endpoint that reuses the existing upgrade flow with the previous application as the target. All three features reuse existing `propagate_upgrade()` and `context_client.update_application()` — no new migration logic.

**Tech Stack:** Rust, actix (actor framework), calimero-store (RocksDB), eyre (error handling), axum (HTTP server)

---

## Prerequisites

- Phases 1–4 complete (storage keys, group CRUD, context-group integration, upgrade propagation)
- All checks pass: `cargo check --workspace`, `cargo fmt --check`, `cargo clippy -- -A warnings`
- Key files from prior phases:
  - `crates/store/src/key/group.rs` — `GroupUpgradeKey`, `GroupUpgradeValue`, `GroupUpgradeStatus`
  - `crates/context/src/group_store.rs` — store helpers (`load_group_meta`, `load_group_upgrade`, `enumerate_group_contexts`, `get_group_for_context`, etc.)
  - `crates/context/src/handlers/upgrade_group.rs` — `propagate_upgrade()` async fn (pub(crate))
  - `crates/context/primitives/src/group.rs` — `UpgradeGroupRequest`, `UpgradeGroupResponse`, etc.

---

## Task 1: Export `GROUP_UPGRADE_PREFIX` for Store Scanning

**Files:**
- Modify: `crates/store/src/key/group.rs:21`
- Modify: `crates/store/src/key.rs:32-35`

Crash recovery (Task 5) needs to iterate all `GroupUpgradeKey` entries in the store. The prefix byte `0x24` is currently private. We need to export it so `group_store.rs` can use it for scanning.

**Step 1: Make the prefix constant public**

In `crates/store/src/key/group.rs`, line 21, change:

```rust
// Before:
const GROUP_UPGRADE_PREFIX: u8 = 0x24;

// After:
pub const GROUP_UPGRADE_PREFIX: u8 = 0x24;
```

**Step 2: Re-export from key.rs**

In `crates/store/src/key.rs`, update the `pub use group::` block (lines 32-35) to include `GROUP_UPGRADE_PREFIX`:

```rust
pub use group::{
    ContextGroupRef, GroupContextIndex, GroupMember, GroupMeta, GroupMetaValue, GroupUpgradeKey,
    GroupUpgradeStatus, GroupUpgradeValue, GROUP_CONTEXT_INDEX_PREFIX, GROUP_MEMBER_PREFIX,
    GROUP_UPGRADE_PREFIX,
};
```

**Step 3: Verify**

```bash
cargo check -p calimero-store
```

Expected: compiles with no errors.

---

## Task 2: Add `enumerate_in_progress_upgrades()` Store Helper

**Files:**
- Modify: `crates/context/src/group_store.rs` (append after `delete_group_upgrade`)

This helper scans all `GroupUpgradeKey` entries and returns those with `InProgress` status — needed for crash recovery.

**Step 1: Add the helper function**

Append after `delete_group_upgrade()` (after line 288) in `crates/context/src/group_store.rs`:

```rust
/// Scans all GroupUpgradeKey entries and returns (group_id, upgrade_value)
/// pairs where status is InProgress. Used for crash recovery on startup.
pub fn enumerate_in_progress_upgrades(
    store: &Store,
) -> EyreResult<Vec<(ContextGroupId, GroupUpgradeValue)>> {
    use calimero_store::key::{AsKeyParts, GroupUpgradeKey, GROUP_UPGRADE_PREFIX};

    let handle = store.handle();
    let start_key = GroupUpgradeKey::new([0u8; 32]);

    let mut iter = handle.iter::<GroupUpgradeKey>()?;
    let first = iter.seek(start_key).transpose();

    let mut results = Vec::new();

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;

        if key.as_key().as_bytes()[0] != GROUP_UPGRADE_PREFIX {
            break;
        }

        let group_id = ContextGroupId::from(key.group_id());

        if let Some(upgrade) = load_group_upgrade(store, &group_id)? {
            if matches!(upgrade.status, GroupUpgradeStatus::InProgress { .. }) {
                results.push((group_id, upgrade));
            }
        }
    }

    Ok(results)
}
```

**Step 2: Add missing import**

At the top of `group_store.rs`, add `GroupUpgradeStatus` to the existing import from `calimero_store::key`:

```rust
use calimero_store::key::{
    AsKeyParts, ContextGroupRef, GroupContextIndex, GroupMember, GroupMeta, GroupMetaValue,
    GroupUpgradeKey, GroupUpgradeStatus, GroupUpgradeValue, GROUP_CONTEXT_INDEX_PREFIX,
    GROUP_MEMBER_PREFIX,
};
```

**Step 3: Verify**

```bash
cargo check -p calimero-context
```

Expected: compiles with no errors.

---

## Task 3: Add `maybe_lazy_upgrade()` Helper in Execute Handler

**Files:**
- Modify: `crates/context/src/handlers/execute.rs:92` (insert after context fetch, before `is_state_op`)

This private function checks if a context belongs to a group with `LazyOnAccess` policy and needs upgrading.

**Step 1: Add new imports**

At the top of `crates/context/src/handlers/execute.rs`, add these imports:

```rust
use calimero_primitives::context::UpgradePolicy;
```

**Step 2: Add the `maybe_lazy_upgrade()` function**

Add this function at the end of the file (before the closing of the module, or after the last function):

```rust
/// Checks if a context belongs to a group with LazyOnAccess policy and
/// the context's current application differs from the group's target.
/// Returns (target_application_id, migrate_method) if an upgrade is needed.
fn maybe_lazy_upgrade(
    datastore: &Store,
    context: &Context,
) -> Option<(ApplicationId, Option<String>)> {
    use crate::group_store;

    // 1. Check if context belongs to a group
    let group_id = match group_store::get_group_for_context(datastore, &context.id) {
        Ok(Some(gid)) => gid,
        Ok(None) => return None, // not in a group
        Err(err) => {
            debug!(%err, context_id=%context.id, "failed to check group for context during lazy upgrade");
            return None;
        }
    };

    // 2. Load group metadata
    let meta = match group_store::load_group_meta(datastore, &group_id) {
        Ok(Some(m)) => m,
        Ok(None) => return None, // group deleted?
        Err(err) => {
            debug!(%err, ?group_id, "failed to load group meta during lazy upgrade");
            return None;
        }
    };

    // 3. Check policy is LazyOnAccess
    if !matches!(meta.upgrade_policy, UpgradePolicy::LazyOnAccess) {
        return None;
    }

    // 4. Compare current vs target application
    if context.application_id == meta.target_application_id {
        return None; // already at target
    }

    // 5. Check if there's an active upgrade with migration info
    let migrate_method = match group_store::load_group_upgrade(datastore, &group_id) {
        Ok(Some(upgrade)) => upgrade
            .migration
            .as_ref()
            .and_then(|bytes| String::from_utf8(bytes.clone()).ok()),
        _ => None,
    };

    info!(
        context_id=%context.id,
        ?group_id,
        current_app=%context.application_id,
        target_app=%meta.target_application_id,
        "lazy upgrade triggered for context"
    );

    Some((meta.target_application_id, migrate_method))
}
```

**Step 3: Verify**

```bash
cargo check -p calimero-context
```

Expected: compiles (function is unused for now — will be wired in Task 4).

---

## Task 4: Wire Lazy Upgrade into Execute Handler

**Files:**
- Modify: `crates/context/src/handlers/execute.rs:84-98`

Insert the lazy upgrade check after context is fetched (line 92) but before the `is_state_op` check (line 94). The upgrade is performed by sending an `UpdateApplication` message through the context client, which is awaited in the async task chain.

**Step 1: Add the lazy upgrade check in the synchronous section**

After line 92 (after `let context = match self.get_or_fetch_context(...)`) and before line 94 (`let is_state_op = ...`), insert:

```rust
        // Lazy upgrade: if context belongs to a LazyOnAccess group and is stale,
        // trigger an upgrade before executing the method.
        let lazy_upgrade_params = maybe_lazy_upgrade(&self.datastore, &context.meta);
```

**Step 2: Clone context_client for the lazy upgrade task**

Before the `guard_task` (line 171), add:

```rust
        let lazy_context_client = lazy_upgrade_params
            .as_ref()
            .map(|_| self.context_client.clone());
```

**Step 3: Insert lazy upgrade task into the async chain**

The lazy upgrade must happen **before** the module is loaded (since the module depends on the application_id). Insert a new task between `guard_task` and `context_task`.

Replace the `context_task` assignment (lines 179-185):

```rust
        // Before:
        let context_task = guard_task.map(move |guard, act, _ctx| {
            let Some(context) = act.get_or_fetch_context(&context_id)? else {
                bail!(ContextError::ContextDeleted { context_id });
            };

            Ok((guard, context.meta.clone()))
        });
```

With:

```rust
        let lazy_upgrade_task = guard_task.map(move |guard, act, _ctx| {
            // If lazy upgrade is needed, send the update_application message
            if let Some((target_app_id, migrate_method)) = lazy_upgrade_params {
                let ctx_client = lazy_context_client.expect("cloned when lazy_upgrade_params is Some");
                info!(
                    %context_id,
                    %target_app_id,
                    "performing lazy upgrade before execution"
                );
                return Ok(Either::Right((guard, ctx_client, context_id, target_app_id, executor, migrate_method)));
            }
            Ok(Either::Left(guard))
        });

        let context_task = lazy_upgrade_task.and_then(move |either, act, _ctx| {
            async move {
                let guard = match either {
                    Either::Left(guard) => guard,
                    Either::Right((guard, ctx_client, cid, target_app, exec, migrate)) => {
                        // Perform the lazy upgrade (awaits completion)
                        if let Err(err) = ctx_client
                            .update_application(&cid, &target_app, &exec, migrate)
                            .await
                        {
                            warn!(
                                %cid,
                                %target_app,
                                %err,
                                "lazy upgrade failed, proceeding with current application"
                            );
                        }
                        guard
                    }
                };
                Ok(guard)
            }
            .into_actor(act)
        });

        // Re-fetch context after possible lazy upgrade (application_id may have changed)
        let context_task = context_task.map(move |guard, act, _ctx| {
            let Some(context) = act.get_or_fetch_context(&context_id)? else {
                bail!(ContextError::ContextDeleted { context_id });
            };

            Ok((guard, context.meta.clone()))
        });
```

**Step 4: Verify**

```bash
cargo check -p calimero-context
cargo clippy -p calimero-context -- -A warnings
```

Expected: compiles with no errors or warnings.

**Important note:** The `Either` type is already imported at line 33 (`use either::Either;`). The `warn!` macro is already imported at line 39. `context_id` and `executor` are `Copy` types so they can be used in both closures.

---

## Task 5: Add `recover_in_progress_upgrades()` to ContextManager

**Files:**
- Modify: `crates/context/src/lib.rs` (add method + startup hook)

This method scans for `GroupUpgradeKey` entries with `InProgress` status and re-spawns `propagate_upgrade()` for each.

**Step 1: Add imports to lib.rs**

Add these imports at the top of `crates/context/src/lib.rs`:

```rust
use calimero_context_config::types::ContextGroupId;
use calimero_store::key::GroupUpgradeStatus;
use tracing::{error, info, warn};
```

**Step 2: Add the recovery method**

Add a new `impl ContextManager` block after the existing `Actor` impl (after line 121):

```rust
impl ContextManager {
    /// Scans the store for in-progress group upgrades and re-spawns
    /// propagators for each. Called during actor startup for crash recovery.
    fn recover_in_progress_upgrades(&self, ctx: &mut actix::Context<Self>) {
        let upgrades = match group_store::enumerate_in_progress_upgrades(&self.datastore) {
            Ok(u) => u,
            Err(err) => {
                error!(?err, "failed to scan for in-progress upgrades during recovery");
                return;
            }
        };

        if upgrades.is_empty() {
            return;
        }

        info!(
            count = upgrades.len(),
            "recovering in-progress group upgrades"
        );

        for (group_id, upgrade) in upgrades {
            let (total, completed, failed) = match upgrade.status {
                GroupUpgradeStatus::InProgress {
                    total,
                    completed,
                    failed,
                } => (total, completed, failed),
                _ => continue, // shouldn't happen given our filter
            };

            info!(
                ?group_id,
                total,
                completed,
                failed,
                "re-spawning propagator for in-progress upgrade"
            );

            // Extract migration method from stored bytes
            let migration = upgrade
                .migration
                .as_ref()
                .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
                .map(|method| {
                    calimero_context_primitives::messages::MigrationParams { method }
                });

            let meta = match group_store::load_group_meta(&self.datastore, &group_id) {
                Ok(Some(m)) => m,
                Ok(None) => {
                    warn!(?group_id, "group not found during recovery, skipping");
                    continue;
                }
                Err(err) => {
                    error!(?group_id, ?err, "failed to load group meta during recovery");
                    continue;
                }
            };

            let propagator = handlers::upgrade_group::propagate_upgrade(
                self.context_client.clone(),
                self.datastore.clone(),
                group_id,
                meta.target_application_id,
                upgrade.initiated_by,
                migration,
                // No canary skip on recovery — propagator's idempotency
                // handles already-upgraded contexts gracefully
                calimero_primitives::context::ContextId::from([0u8; 32]),
                total as usize,
            );

            ctx.spawn(
                propagator.into_actor(self)
            );
        }
    }
}
```

**Step 3: Wire into Actor startup**

Replace the existing `Actor` impl (lines 119-121):

```rust
// Before:
impl Actor for ContextManager {
    type Context = actix::Context<Self>;
}

// After:
impl Actor for ContextManager {
    type Context = actix::Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.recover_in_progress_upgrades(ctx);
    }
}
```

**Step 4: Add required trait import**

Add `use actix::AsyncContext;` at the top of `lib.rs` (needed for `ctx.spawn()`):

```rust
use actix::{Actor, AsyncContext};
```

Also add `WrapFuture` for `.into_actor()`:

```rust
use actix::{Actor, AsyncContext, WrapFuture};
```

**Step 5: Verify**

```bash
cargo check -p calimero-context
```

Expected: compiles with no errors.

---

## Task 6: Add Rollback Message Type + ContextClient Method

**Files:**
- Modify: `crates/context/primitives/src/group.rs` (append after `RetryGroupUpgradeRequest`)
- Modify: `crates/context/primitives/src/messages.rs` (add `RollbackGroup` variant)
- Modify: `crates/context/primitives/src/client.rs` (add `rollback_group()` method)

**Step 1: Add request type to group.rs**

Append at the end of `crates/context/primitives/src/group.rs` (after line 148):

```rust
#[derive(Debug)]
pub struct RollbackGroupRequest {
    pub group_id: ContextGroupId,
    pub requester: PublicKey,
    pub reason: Option<String>,
}

impl Message for RollbackGroupRequest {
    type Result = eyre::Result<UpgradeGroupResponse>;
}
```

**Step 2: Add ContextMessage variant to messages.rs**

In `crates/context/primitives/src/messages.rs`, add `RollbackGroupRequest` to the import from `crate::group`:

```rust
use crate::group::{
    ..., RollbackGroupRequest,
};
```

Then add the variant to `ContextMessage` enum (after `RetryGroupUpgrade`):

```rust
    RollbackGroup {
        request: RollbackGroupRequest,
        outcome: oneshot::Sender<<RollbackGroupRequest as Message>::Result>,
    },
```

**Step 3: Add ContextClient method to client.rs**

In `crates/context/primitives/src/client.rs`, add `RollbackGroupRequest` to the imports from `crate::group`:

```rust
use crate::group::{
    ..., RollbackGroupRequest,
};
```

Then add the method (after `retry_group_upgrade()`):

```rust
    pub async fn rollback_group(
        &self,
        request: RollbackGroupRequest,
    ) -> eyre::Result<UpgradeGroupResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::RollbackGroup {
                request,
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }
```

**Step 4: Verify**

```bash
cargo check -p calimero-context-primitives
```

Expected: compiles (handler not wired yet — will warn about unused variant, that's fine).

---

## Task 7: Implement Rollback Handler

**Files:**
- Create: `crates/context/src/handlers/rollback_group.rs`
- Modify: `crates/context/src/handlers.rs` (add module + match arm)

**Step 1: Create the handler file**

Create `crates/context/src/handlers/rollback_group.rs`:

```rust
use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorResponse, AsyncContext, Handler, Message, WrapFuture};
use calimero_context_primitives::group::{RollbackGroupRequest, UpgradeGroupResponse};
use calimero_context_primitives::messages::MigrationParams;
use calimero_primitives::context::ContextId;
use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};
use eyre::bail;
use tracing::{error, info};

use crate::{group_store, ContextManager};

impl Handler<RollbackGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <RollbackGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        RollbackGroupRequest {
            group_id,
            requester,
            reason,
        }: RollbackGroupRequest,
        ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            // 1. Requester must be admin
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            // 2. Load current upgrade — must exist and be Completed or InProgress with failures
            let upgrade = group_store::load_group_upgrade(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("no upgrade found for this group"))?;

            match &upgrade.status {
                GroupUpgradeStatus::RolledBack { .. } => {
                    bail!("upgrade is already rolled back");
                }
                GroupUpgradeStatus::InProgress { failed: 0, .. } => {
                    bail!("cannot rollback while upgrade is actively in progress with no failures");
                }
                _ => {} // Completed or InProgress with failures — both valid for rollback
            }

            // 3. Load group meta to get the previous application_id
            let meta = group_store::load_group_meta(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            // The "from" application is stored in the upgrade record conceptually,
            // but we stored revision numbers (0, 0) — for rollback, we need the
            // application_id that was the target BEFORE the upgrade changed it.
            // Since the group meta's target_application_id was updated during upgrade,
            // and we don't separately store the old application_id, the caller needs
            // to supply the rollback target via a new upgrade with the old app id.
            //
            // For now: mark current upgrade as RolledBack and let the admin
            // start a new upgrade with the desired application_id.

            let reason_str = reason.unwrap_or_else(|| "admin-initiated rollback".to_owned());

            let mut rolled_back_upgrade = upgrade;
            rolled_back_upgrade.status = GroupUpgradeStatus::RolledBack {
                reason: reason_str,
            };
            group_store::save_group_upgrade(
                &self.datastore,
                &group_id,
                &rolled_back_upgrade,
            )?;

            info!(
                ?group_id,
                %requester,
                "group upgrade marked as rolled back"
            );

            Ok(rolled_back_upgrade.status)
        })();

        match result {
            Ok(status) => ActorResponse::reply(Ok(UpgradeGroupResponse { group_id, status })),
            Err(err) => ActorResponse::reply(Err(err)),
        }
    }
}
```

**Step 2: Wire into handlers.rs**

In `crates/context/src/handlers.rs`, add the module declaration (after `pub mod retry_group_upgrade;`):

```rust
pub mod rollback_group;
```

Add the match arm (after `RetryGroupUpgrade`):

```rust
            ContextMessage::RollbackGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
```

**Step 3: Verify**

```bash
cargo check -p calimero-context
```

Expected: compiles with no errors.

---

## Task 8: Add Server API Types for Rollback

**Files:**
- Modify: `crates/server/primitives/src/admin.rs` (append after `RetryGroupUpgradeApiRequest`)

**Step 1: Add the API request type**

Append after line 2008 in `crates/server/primitives/src/admin.rs`:

```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackGroupApiRequest {
    pub requester: PublicKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Validate for RollbackGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}
```

Note: The rollback response reuses `UpgradeGroupApiResponse` (same structure — group_id + status).

**Step 2: Verify**

```bash
cargo check -p calimero-server-primitives
```

Expected: compiles with no errors.

---

## Task 9: Create Server Rollback Handler + Route

**Files:**
- Create: `crates/server/src/admin/handlers/groups/rollback_group.rs`
- Modify: `crates/server/src/admin/handlers/groups.rs` (add module)
- Modify: `crates/server/src/admin/service.rs` (add route)

**Step 1: Create the server handler**

Create `crates/server/src/admin/handlers/groups/rollback_group.rs`:

```rust
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_context_primitives::group::RollbackGroupRequest;
use calimero_server_primitives::admin::{
    RollbackGroupApiRequest, UpgradeGroupApiResponse, UpgradeGroupApiResponseData,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<RollbackGroupApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Initiating group upgrade rollback");

    let result = state
        .ctx_client
        .rollback_group(RollbackGroupRequest {
            group_id,
            requester: req.requester,
            reason: req.reason,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            let (status_str, total, completed, failed) =
                super::upgrade_group::format_status(&resp.status);
            info!(group_id=%group_id_str, %status_str, "Group upgrade rolled back");
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
            error!(group_id=%group_id_str, error=?err, "Failed to rollback group upgrade");
            err.into_response()
        }
    }
}
```

**Step 2: Add module declaration**

In `crates/server/src/admin/handlers/groups.rs`, add after `pub mod retry_group_upgrade;`:

```rust
pub mod rollback_group;
```

**Step 3: Add route**

In `crates/server/src/admin/service.rs`, add after the retry route (after line 252):

```rust
        .route(
            "/groups/:group_id/upgrade/rollback",
            post(groups::rollback_group::handler),
        )
```

**Step 4: Verify**

```bash
cargo check --workspace
cargo fmt --check
cargo clippy -- -A warnings
```

Expected: all pass.

---

## Task 10: Update Implementation Plan Doc

**Files:**
- Modify: `CONTEXT_GROUPS_IMPL_PLAN.md`

**Step 1: Mark Phase 5 tasks as completed**

Update the following task IDs from `status: pending` to `status: completed`:
- `p5-lazy-helper` (P5.1)
- `p5-lazy-wire` (P5.2)
- `p5-crash-recovery` (P5.3)
- `p5-rollback-endpoint` (P5.4)
- `p5-ctx-doc-final` (P5.5)

**Step 2: Update Phase 5 section**

In the Phase 5 section, update the files table to reflect actual implementation.

**Step 3: Final verification**

```bash
cargo check --workspace
cargo fmt --check
cargo clippy -- -A warnings
```

Expected: all pass.

---

## Execution Batches

| Batch | Tasks | Focus |
|-------|-------|-------|
| 1 | 1, 2, 3 | Foundation: export prefix, store scanner, lazy upgrade helper |
| 2 | 4, 5 | Core wiring: lazy upgrade in execute path, crash recovery on startup |
| 3 | 6, 7 | Rollback: message types + handler |
| 4 | 8, 9, 10 | Server endpoint + docs |

## Verification Checklist

After all batches:
- [ ] `cargo check --workspace` passes
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -A warnings` passes
- [ ] Lazy upgrade check is inserted before module loading in execute handler
- [ ] `maybe_lazy_upgrade()` returns `None` for non-group contexts (no overhead)
- [ ] `maybe_lazy_upgrade()` returns `None` for non-LazyOnAccess policies
- [ ] `recover_in_progress_upgrades()` is called from `ContextManager::started()`
- [ ] Recovery spawns propagators with `skip_context = ContextId::zero()` (no canary skip)
- [ ] Rollback marks upgrade as `RolledBack` with reason
- [ ] New route `POST /groups/:group_id/upgrade/rollback` registered
- [ ] No dead code, no unused imports

## New API Route (Phase 5)

```
POST   /admin-api/groups/:group_id/upgrade/rollback  → RollbackGroup
```
