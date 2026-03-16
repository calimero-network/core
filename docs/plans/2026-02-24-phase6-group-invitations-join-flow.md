# Phase 6 — Group Invitations + Join Flow Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let admins generate invitation payloads that users can present to join a group, mirroring the existing `ContextInvitationPayload` pattern. Supports both targeted (specific invitee) and open (anyone) invitations with optional expiration.

**Architecture:** A `GroupInvitationPayload` newtype in primitives (borsh-serialized inner struct, base58-encoded outer string), two new handlers (`create_group_invitation` and `join_group`), corresponding server endpoints. The join handler reuses existing `add_group_member()` from `group_store.rs`. No new on-chain contract changes — the existing `GroupRequest::AddMembers` handles on-chain registration.

**Tech Stack:** Rust, actix (actor framework), calimero-store (RocksDB), eyre (error handling), axum (HTTP server), borsh (binary serialization), bs58 (base58 encoding)

---

## Prerequisites

- Phases 1–5 complete (storage keys, group CRUD, context-group integration, upgrade propagation, advanced policies)
- All checks pass: `cargo check --workspace`, `cargo fmt --check`, `cargo clippy -- -A warnings`
- Key files from prior phases:
  - `crates/primitives/src/context.rs` — `ContextInvitationPayload` (pattern to follow), `GroupMemberRole`
  - `crates/context/src/group_store.rs` — `add_group_member`, `require_group_admin`, `is_group_admin`, `check_group_membership`, `load_group_meta`
  - `crates/context/primitives/src/group.rs` — existing group message types
  - `crates/context/primitives/src/messages.rs` — `ContextMessage` enum (line 166)
  - `crates/context/primitives/src/client.rs` — `ContextClient` group operations (line 980+)
  - `crates/context/src/handlers.rs` — handler routing (line 25)
  - `crates/server/src/admin/handlers/groups.rs` — server handler modules (line 1)
  - `crates/server/src/admin/service.rs` — route registration (lines 221–252)
  - `crates/server/primitives/src/admin.rs` — API request/response types (line 1999+)

---

## Task 1: Add `GroupInvitationPayload` Type to Primitives

**Files:**
- Modify: `crates/primitives/src/context.rs` (append after `GroupMemberRole`, before `#[cfg(test)]`)

This is the core data type. It follows the exact same pattern as `ContextInvitationPayload` (lines 100–235 of `context.rs`): a newtype wrapping `Vec<u8>` with borsh inner serialization and base58 outer encoding.

**Step 1: Add the newtype and its Display/FromStr/From/TryFrom impls**

Append after the `GroupMemberRole` definition (after line 375), before the `#[cfg(test)]` block (line 377):

```rust
/// A serialized and encoded payload for inviting a user to join a Context Group.
///
/// Internally Borsh-serialized for compact, deterministic representation and
/// then Base58-encoded for a human-readable string format.
/// Supports both targeted invitations (specific invitee) and open invitations (anyone can redeem).
#[derive(Clone, Serialize, Deserialize)]
#[serde(into = "String", try_from = "&str")]
pub struct GroupInvitationPayload(Vec<u8>);

impl fmt::Debug for GroupInvitationPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("GroupInvitationPayload");
        _ = d.field("raw", &self.to_string());
        d.finish()
    }
}

impl fmt::Display for GroupInvitationPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&bs58::encode(self.0.as_slice()).into_string())
    }
}

impl FromStr for GroupInvitationPayload {
    type Err = io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        bs58::decode(s)
            .into_vec()
            .map(Self)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }
}

impl From<GroupInvitationPayload> for String {
    fn from(payload: GroupInvitationPayload) -> Self {
        bs58::encode(payload.0.as_slice()).into_string()
    }
}

impl TryFrom<&str> for GroupInvitationPayload {
    type Error = io::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}
```

**Step 2: Add the borsh-gated `new()` and `parts()` methods**

Append immediately after the `TryFrom` impl:

```rust
#[cfg(feature = "borsh")]
const _: () = {
    use borsh::{BorshDeserialize, BorshSerialize};

    use crate::identity::PublicKey;

    #[derive(BorshSerialize, BorshDeserialize)]
    struct GroupInvitationInner {
        group_id: [u8; DIGEST_SIZE],
        inviter_identity: [u8; DIGEST_SIZE],
        invitee_identity: Option<[u8; DIGEST_SIZE]>,
        expiration: Option<u64>,
    }

    impl GroupInvitationPayload {
        /// Creates a new, serialized group invitation payload.
        ///
        /// # Arguments
        /// * `group_id` - The 32-byte group identifier.
        /// * `inviter_identity` - The public key of the admin who created the invitation.
        /// * `invitee_identity` - Optional specific invitee. `None` means open invitation.
        /// * `expiration` - Optional unix timestamp after which the invitation is invalid.
        pub fn new(
            group_id: [u8; DIGEST_SIZE],
            inviter_identity: PublicKey,
            invitee_identity: Option<PublicKey>,
            expiration: Option<u64>,
        ) -> io::Result<Self> {
            let payload = GroupInvitationInner {
                group_id,
                inviter_identity: *inviter_identity,
                invitee_identity: invitee_identity.map(|pk| *pk),
                expiration,
            };

            borsh::to_vec(&payload).map(Self)
        }

        /// Deserializes the payload and extracts its constituent parts.
        ///
        /// # Returns
        /// A tuple of `(group_id_bytes, inviter_identity, invitee_identity, expiration)`.
        pub fn parts(
            &self,
        ) -> io::Result<([u8; DIGEST_SIZE], PublicKey, Option<PublicKey>, Option<u64>)> {
            let payload: GroupInvitationInner = borsh::from_slice(&self.0)?;

            Ok((
                payload.group_id,
                payload.inviter_identity.into(),
                payload.invitee_identity.map(Into::into),
                payload.expiration,
            ))
        }
    }
};
```

**Step 3: Add a roundtrip test**

Inside the existing `#[cfg(test)] mod tests` block, append:

```rust
    #[test]
    fn test_group_invitation_payload_roundtrip_targeted() {
        let group_id = [3u8; DIGEST_SIZE];
        let inviter = PublicKey::from([4; DIGEST_SIZE]);
        let invitee = PublicKey::from([5; DIGEST_SIZE]);

        let payload = GroupInvitationPayload::new(group_id, inviter, Some(invitee), Some(1_700_000_000))
            .expect("Payload creation should succeed");

        let encoded = payload.to_string();
        assert!(!encoded.is_empty());

        let decoded = GroupInvitationPayload::from_str(&encoded)
            .expect("Payload decoding should succeed");

        let (g, inv, invitee_out, exp) = decoded.parts().expect("Parts extraction should succeed");
        assert_eq!(g, group_id);
        assert_eq!(inv, inviter);
        assert_eq!(invitee_out, Some(invitee));
        assert_eq!(exp, Some(1_700_000_000));
    }

    #[test]
    fn test_group_invitation_payload_roundtrip_open() {
        let group_id = [6u8; DIGEST_SIZE];
        let inviter = PublicKey::from([7; DIGEST_SIZE]);

        let payload = GroupInvitationPayload::new(group_id, inviter, None, None)
            .expect("Payload creation should succeed");

        let encoded = payload.to_string();
        let decoded = GroupInvitationPayload::from_str(&encoded)
            .expect("Payload decoding should succeed");

        let (g, inv, invitee_out, exp) = decoded.parts().expect("Parts extraction should succeed");
        assert_eq!(g, group_id);
        assert_eq!(inv, inviter);
        assert_eq!(invitee_out, None);
        assert_eq!(exp, None);
    }

    #[test]
    fn test_group_invitation_payload_invalid_base58() {
        let result = GroupInvitationPayload::from_str("This is not valid Base58!");
        assert!(result.is_err());
    }
```

**Step 4: Verify**

```bash
cargo test -p calimero-primitives --all-features
cargo check -p calimero-primitives
```

Expected: tests pass, compiles with no errors.

---

## Task 2: Add Invitation + Join Message Types to group.rs

**Files:**
- Modify: `crates/context/primitives/src/group.rs` (append after `RetryGroupUpgradeRequest`)

**Step 1: Add import for `GroupInvitationPayload`**

At the top of `crates/context/primitives/src/group.rs`, add `GroupInvitationPayload` to the existing import from `calimero_primitives::context`:

```rust
use calimero_primitives::context::{ContextId, GroupInvitationPayload, GroupMemberRole, UpgradePolicy};
```

**Step 2: Add request/response types**

Append after line 148 (`RetryGroupUpgradeRequest` Message impl):

```rust
#[derive(Debug)]
pub struct CreateGroupInvitationRequest {
    pub group_id: ContextGroupId,
    pub requester: PublicKey,
    pub invitee_identity: Option<PublicKey>,
    pub expiration: Option<u64>,
}

impl Message for CreateGroupInvitationRequest {
    type Result = eyre::Result<CreateGroupInvitationResponse>;
}

#[derive(Debug)]
pub struct CreateGroupInvitationResponse {
    pub payload: GroupInvitationPayload,
}

#[derive(Debug)]
pub struct JoinGroupRequest {
    pub invitation_payload: GroupInvitationPayload,
    pub joiner_identity: PublicKey,
}

impl Message for JoinGroupRequest {
    type Result = eyre::Result<JoinGroupResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct JoinGroupResponse {
    pub group_id: ContextGroupId,
    pub member_identity: PublicKey,
}
```

**Step 3: Verify**

```bash
cargo check -p calimero-context-primitives
```

Expected: compiles (types unused for now — will be wired in Tasks 3–4).

---

## Task 3: Add ContextMessage Variants + ContextClient Methods

**Files:**
- Modify: `crates/context/primitives/src/messages.rs` (add variants to `ContextMessage` enum)
- Modify: `crates/context/primitives/src/client.rs` (add client methods)

**Step 1: Add imports in messages.rs**

In `crates/context/primitives/src/messages.rs` line 12-16, add `CreateGroupInvitationRequest` and `JoinGroupRequest` to the import from `crate::group`:

```rust
use crate::group::{
    AddGroupMembersRequest, CreateGroupInvitationRequest, CreateGroupRequest, DeleteGroupRequest,
    GetGroupInfoRequest, GetGroupUpgradeStatusRequest, JoinGroupRequest, ListGroupContextsRequest,
    ListGroupMembersRequest, RemoveGroupMembersRequest, RetryGroupUpgradeRequest,
    UpgradeGroupRequest,
};
```

**Step 2: Add ContextMessage variants**

In messages.rs, add these two variants after `RetryGroupUpgrade` (after line 234, before the closing `}`):

```rust
    CreateGroupInvitation {
        request: CreateGroupInvitationRequest,
        outcome: oneshot::Sender<<CreateGroupInvitationRequest as Message>::Result>,
    },
    JoinGroup {
        request: JoinGroupRequest,
        outcome: oneshot::Sender<<JoinGroupRequest as Message>::Result>,
    },
```

**Step 3: Add imports in client.rs**

In `crates/context/primitives/src/client.rs` lines 28-33, add the new types to the import from `crate::group`:

```rust
use crate::group::{
    AddGroupMembersRequest, CreateGroupInvitationRequest, CreateGroupInvitationResponse,
    CreateGroupRequest, CreateGroupResponse, DeleteGroupRequest, DeleteGroupResponse,
    GetGroupInfoRequest, GetGroupUpgradeStatusRequest, GroupInfoResponse, GroupMemberEntry,
    JoinGroupRequest, JoinGroupResponse, ListGroupContextsRequest, ListGroupMembersRequest,
    RemoveGroupMembersRequest, RetryGroupUpgradeRequest, UpgradeGroupRequest, UpgradeGroupResponse,
};
```

**Step 4: Add ContextClient methods**

In client.rs, append these methods after `retry_group_upgrade()` (after line 1147, before the closing `}`):

```rust
    pub async fn create_group_invitation(
        &self,
        request: CreateGroupInvitationRequest,
    ) -> eyre::Result<CreateGroupInvitationResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::CreateGroupInvitation {
                request,
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    pub async fn join_group(
        &self,
        request: JoinGroupRequest,
    ) -> eyre::Result<JoinGroupResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::JoinGroup {
                request,
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }
```

**Step 5: Verify**

```bash
cargo check -p calimero-context-primitives
```

Expected: compiles (handler not wired yet — unused variant warnings are fine).

---

## Task 4: Create `create_group_invitation.rs` Handler

**Files:**
- Create: `crates/context/src/handlers/create_group_invitation.rs`

**Step 1: Create the handler file**

Create `crates/context/src/handlers/create_group_invitation.rs`:

```rust
use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::{
    CreateGroupInvitationRequest, CreateGroupInvitationResponse,
};
use calimero_primitives::context::GroupInvitationPayload;

use crate::{group_store, ContextManager};

impl Handler<CreateGroupInvitationRequest> for ContextManager {
    type Result = ActorResponse<Self, <CreateGroupInvitationRequest as Message>::Result>;

    fn handle(
        &mut self,
        CreateGroupInvitationRequest {
            group_id,
            requester,
            invitee_identity,
            expiration,
        }: CreateGroupInvitationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            // 1. Group must exist
            let _meta = group_store::load_group_meta(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            // 2. Requester must be admin
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            // 3. Validate expiration (if set, must be in the future)
            if let Some(exp) = expiration {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if exp <= now {
                    eyre::bail!("expiration must be a future unix timestamp");
                }
            }

            // 4. Build the invitation payload
            let payload = GroupInvitationPayload::new(
                group_id.to_bytes(),
                requester,
                invitee_identity,
                expiration,
            )?;

            Ok(CreateGroupInvitationResponse { payload })
        })();

        ActorResponse::reply(result)
    }
}
```

**Step 2: Verify**

```bash
cargo check -p calimero-context
```

Expected: compiles (handler module not registered yet — will be wired in Task 6).

---

## Task 5: Create `join_group.rs` Handler

**Files:**
- Create: `crates/context/src/handlers/join_group.rs`

This is the most complex handler. It decodes the invitation, validates the inviter is still an admin, checks targeting and expiration, then adds the joiner as a Member.

**Step 1: Create the handler file**

Create `crates/context/src/handlers/join_group.rs`:

```rust
use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorResponse, Handler, Message};
use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_primitives::context::GroupMemberRole;
use eyre::bail;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<JoinGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinGroupRequest {
            invitation_payload,
            joiner_identity,
        }: JoinGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            // 1. Decode the invitation payload
            let (group_id_bytes, inviter_identity, invitee_identity, expiration) =
                invitation_payload.parts().map_err(|err| {
                    eyre::eyre!("failed to decode group invitation payload: {err}")
                })?;

            let group_id = ContextGroupId::from(group_id_bytes);

            // 2. Group must exist
            let _meta = group_store::load_group_meta(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            // 3. Inviter must still be an admin
            if !group_store::is_group_admin(&self.datastore, &group_id, &inviter_identity)? {
                bail!("inviter is no longer an admin of this group");
            }

            // 4. If targeted invitation, verify joiner matches invitee
            if let Some(expected_invitee) = invitee_identity {
                if expected_invitee != joiner_identity {
                    bail!("this invitation is for a different identity");
                }
            }

            // 5. Check expiration
            if let Some(exp) = expiration {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if now > exp {
                    bail!("invitation has expired");
                }
            }

            // 6. Check if joiner is already a member
            if group_store::check_group_membership(
                &self.datastore,
                &group_id,
                &joiner_identity,
            )? {
                bail!("identity is already a member of this group");
            }

            // 7. Add joiner as Member (not Admin)
            group_store::add_group_member(
                &self.datastore,
                &group_id,
                &joiner_identity,
                GroupMemberRole::Member,
            )?;

            info!(
                ?group_id,
                %joiner_identity,
                %inviter_identity,
                "new member joined group via invitation"
            );

            Ok(JoinGroupResponse {
                group_id,
                member_identity: joiner_identity,
            })
        })();

        ActorResponse::reply(result)
    }
}
```

**Step 2: Verify**

```bash
cargo check -p calimero-context
```

Expected: compiles (handler module not registered yet — will be wired in Task 6).

---

## Task 6: Wire Handlers into `handlers.rs`

**Files:**
- Modify: `crates/context/src/handlers.rs` (add module declarations + match arms)

**Step 1: Add module declarations**

In `crates/context/src/handlers.rs`, add these two module declarations (after line 8, `pub mod create_group;`):

```rust
pub mod create_group_invitation;
```

And after line 15, `pub mod join_context;`:

```rust
pub mod join_group;
```

(Alphabetical order within the module list.)

**Step 2: Add match arms**

In the `Handler<ContextMessage>` impl, add these arms after `RetryGroupUpgrade` (after line 84, before the closing `}`):

```rust
            ContextMessage::CreateGroupInvitation { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::JoinGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
```

**Step 3: Verify**

```bash
cargo check -p calimero-context
```

Expected: compiles with no errors.

---

## Task 7: Add Server API Types

**Files:**
- Modify: `crates/server/primitives/src/admin.rs` (append after `RetryGroupUpgradeApiRequest`)

**Step 1: Add API request/response types**

Append after line 2007 in `crates/server/primitives/src/admin.rs`:

```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInvitationApiRequest {
    pub requester: PublicKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invitee_identity: Option<PublicKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration: Option<u64>,
}

impl Validate for CreateGroupInvitationApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInvitationApiResponse {
    pub data: CreateGroupInvitationApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInvitationApiResponseData {
    pub payload: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinGroupApiRequest {
    pub invitation_payload: String,
    pub joiner_identity: PublicKey,
}

impl Validate for JoinGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        if self.invitation_payload.is_empty() {
            return vec![ValidationError::EmptyField {
                field: "invitation_payload",
            }];
        }
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinGroupApiResponse {
    pub data: JoinGroupApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinGroupApiResponseData {
    pub group_id: String,
    pub member_identity: PublicKey,
}
```

**Step 2: Verify**

```bash
cargo check -p calimero-server-primitives
```

Expected: compiles with no errors.

---

## Task 8: Create Server Handlers + Register Routes

**Files:**
- Create: `crates/server/src/admin/handlers/groups/create_group_invitation.rs`
- Create: `crates/server/src/admin/handlers/groups/join_group.rs`
- Modify: `crates/server/src/admin/handlers/groups.rs` (add module declarations)
- Modify: `crates/server/src/admin/service.rs` (add routes)

**Step 1: Create `create_group_invitation.rs` server handler**

Create `crates/server/src/admin/handlers/groups/create_group_invitation.rs`:

```rust
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::CreateGroupInvitationRequest;
use calimero_server_primitives::admin::{
    CreateGroupInvitationApiRequest, CreateGroupInvitationApiResponse,
    CreateGroupInvitationApiResponseData,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<CreateGroupInvitationApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Creating group invitation");

    let result = state
        .ctx_client
        .create_group_invitation(CreateGroupInvitationRequest {
            group_id,
            requester: req.requester,
            invitee_identity: req.invitee_identity,
            expiration: req.expiration,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(group_id=%group_id_str, "Group invitation created");
            ApiResponse {
                payload: CreateGroupInvitationApiResponse {
                    data: CreateGroupInvitationApiResponseData {
                        payload: resp.payload.to_string(),
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to create group invitation");
            err.into_response()
        }
    }
}
```

**Step 2: Create `join_group.rs` server handler**

Create `crates/server/src/admin/handlers/groups/join_group.rs`:

```rust
use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::JoinGroupRequest;
use calimero_primitives::context::GroupInvitationPayload;
use calimero_server_primitives::admin::{
    JoinGroupApiRequest, JoinGroupApiResponse, JoinGroupApiResponseData,
};
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<JoinGroupApiRequest>,
) -> impl IntoResponse {
    let invitation_payload: GroupInvitationPayload = match req.invitation_payload.parse() {
        Ok(p) => p,
        Err(err) => {
            return crate::admin::service::ApiError {
                status_code: reqwest::StatusCode::BAD_REQUEST,
                message: format!("Invalid invitation payload: {err}"),
            }
            .into_response();
        }
    };

    info!(joiner=%req.joiner_identity, "Joining group via invitation");

    let result = state
        .ctx_client
        .join_group(JoinGroupRequest {
            invitation_payload,
            joiner_identity: req.joiner_identity,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            let group_id_hex = hex::encode(resp.group_id.to_bytes());
            info!(group_id=%group_id_hex, member=%resp.member_identity, "Joined group successfully");
            ApiResponse {
                payload: JoinGroupApiResponse {
                    data: JoinGroupApiResponseData {
                        group_id: group_id_hex,
                        member_identity: resp.member_identity,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to join group");
            err.into_response()
        }
    }
}
```

**Step 3: Add module declarations in groups.rs**

In `crates/server/src/admin/handlers/groups.rs`, add these module declarations (in alphabetical order):

After line 1 (`pub mod add_group_members;`), add:
```rust
pub mod create_group_invitation;
```

After `pub mod get_group_info;` (line 4), add:
```rust
pub mod join_group;
```

**Step 4: Add routes in service.rs**

In `crates/server/src/admin/service.rs`, add these routes after the retry route (after line 252, before `// Alias management`):

```rust
        .route(
            "/groups/:group_id/invite",
            post(groups::create_group_invitation::handler),
        )
        .route(
            "/groups/join",
            post(groups::join_group::handler),
        )
```

**Step 5: Verify**

```bash
cargo check --workspace
cargo fmt --check
cargo clippy -- -A warnings
```

Expected: all pass.

---

## Task 9: Update Implementation Plan Doc

**Files:**
- Modify: `CONTEXT_GROUPS_IMPL_PLAN.md`

**Step 1: Mark Phase 6 tasks as completed**

Update the following task IDs from `status: pending` to `status: completed`:
- `p6-invitation-payload-type` (P6.1)
- `p6-invitation-payload-impl` (P6.2)
- `p6-prim-invite-requests` (P6.3)
- `p6-prim-join-request` (P6.4)
- `p6-msg-enum` (P6.5)
- `p6-handler-create-invitation` (P6.6)
- `p6-handler-join-group` (P6.7)
- `p6-handlers-rs` (P6.8)
- `p6-server-invite-endpoint` (P6.9)
- `p6-server-service` (P6.10)
- `p6-ctx-doc-final` (P6.11)

**Step 2: Update the Phase 6 file table entries**

In the Full File Change Table, update the Phase 6 entries from `⬜` to `✅`, and add any missing rows for new files:

```
| P6    | `primitives/src/context.rs`                       | Modify  | +80      | ✅      |
| P6    | `context/primitives/src/group.rs`                 | Modify  | +35      | ✅      |
| P6    | `context/primitives/src/messages.rs`               | Modify  | +10      | ✅      |
| P6    | `context/primitives/src/client.rs`                 | Modify  | +35      | ✅      |
| P6    | `context/src/handlers/create_group_invitation.rs` | **New** | ~50      | ✅      |
| P6    | `context/src/handlers/join_group.rs`              | **New** | ~85      | ✅      |
| P6    | `context/src/handlers.rs`                          | Modify  | +10      | ✅      |
| P6    | `server/primitives/src/admin.rs`                   | Modify  | +60      | ✅      |
| P6    | `server/src/admin/handlers/groups.rs`              | Modify  | +2       | ✅      |
| P6    | `server/src/admin/handlers/groups/create_group_invitation.rs` | **New** | ~60 | ✅ |
| P6    | `server/src/admin/handlers/groups/join_group.rs`   | **New** | ~65      | ✅      |
| P6    | `server/src/admin/service.rs`                      | Modify  | +8       | ✅      |
```

**Step 3: Final verification**

```bash
cargo check --workspace
cargo fmt --check
cargo clippy -- -A warnings
cargo test -p calimero-primitives --all-features
```

Expected: all pass.

---

## Execution Batches

| Batch | Tasks | Focus |
|-------|-------|-------|
| 1 | 1, 2, 3 | Foundation: `GroupInvitationPayload` type, message types, client methods |
| 2 | 4, 5, 6 | Core: create_group_invitation + join_group handlers, handler wiring |
| 3 | 7, 8, 9 | Server: API types, server handlers, routes, docs |

## Verification Checklist

After all batches:
- [ ] `cargo check --workspace` passes
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -A warnings` passes
- [ ] `cargo test -p calimero-primitives --all-features` passes (roundtrip tests for GroupInvitationPayload)
- [ ] `GroupInvitationPayload::new()` + `parts()` roundtrip preserves all fields
- [ ] Open invitation (invitee=None) roundtrip works
- [ ] Targeted invitation (invitee=Some) roundtrip works
- [ ] `create_group_invitation` requires admin
- [ ] `create_group_invitation` validates expiration is in the future
- [ ] `join_group` verifies inviter is still admin
- [ ] `join_group` rejects expired invitations
- [ ] `join_group` rejects targeted invitation for wrong identity
- [ ] `join_group` rejects already-member
- [ ] New routes registered: `POST /groups/:group_id/invite`, `POST /groups/join`
- [ ] No dead code, no unused imports

## New API Routes (Phase 6)

```
POST  /admin-api/groups/:group_id/invite   → CreateGroupInvitation  (admin only)
POST  /admin-api/groups/join               → JoinGroup              (any identity with valid invitation)
```

### Example: Create Invitation (targeted)

```bash
curl -X POST http://localhost:2428/admin-api/groups/<group_id>/invite \
  -H "Content-Type: application/json" \
  -d '{
    "requester": "<admin_public_key>",
    "inviteeIdentity": "<specific_public_key>",
    "expiration": 1700000000
  }'
# Response: { "data": { "payload": "3xFg7k...base58..." } }
```

### Example: Create Invitation (open)

```bash
curl -X POST http://localhost:2428/admin-api/groups/<group_id>/invite \
  -H "Content-Type: application/json" \
  -d '{
    "requester": "<admin_public_key>"
  }'
# Response: { "data": { "payload": "3xFg7k...base58..." } }
```

### Example: Join Group

```bash
curl -X POST http://localhost:2428/admin-api/groups/join \
  -H "Content-Type: application/json" \
  -d '{
    "invitationPayload": "3xFg7k...base58...",
    "joinerIdentity": "<public_key>"
  }'
# Response: { "data": { "groupId": "...", "memberIdentity": "<public_key>" } }
```
