# Group join via invitation — contract alignment with context invitation

## How context invitation works in the contract

Context invitation in `context-config` is a **two-step, invitee-driven** flow so that the **new member** (or a relayer) can submit the on-chain transaction; no existing member has to submit for them.

### Step 1: Commit (`commit_invitation`)

- **Entry:** `mutate()` with `ContextRequestKind::CommitOpenInvitation { commitment_hash, expiration_block_height }`.
- **Who can call:** Anyone (invitee or relayer). The request is still signed (for NEAR auth), but the contract does **not** check that the signer is a context member.
- **Contract:** Asserts context exists, commitment hash not already used, `block_height < expiration_block_height`, then stores `commitments_open_invitations.insert(hash, expiration_block_height)`.
- **Purpose:** Commit to a hash of the future reveal payload (MEV / replay protection).

### Step 2: Reveal (`reveal_invitation`)

- **Entry:** `mutate()` with `ContextRequestKind::RevealOpenInvitation { payload }` where `payload: SignedRevealPayload`.
- **Who can call:** Anyone (typically the invitee or relayer). Authorization is proved **inside the payload**, not by the transaction signer.
- **Contract:**
  1. Verifies protocol/contract match, context exists.
  2. Hashes payload data, finds and **removes** the commitment (replay protection).
  3. Checks invitation not expired, invitee not already a member.
  4. Verifies **invitee’s** signature over the payload data (proves invitee consented).
  5. Verifies **inviter’s** signature over the invitation (proves inviter authorized).
  6. Marks inviter’s signature as used (per-inviter replay set).
  7. **Adds** `payload_data.new_member_identity` to `context.members`.
- **Result:** The **invitee** is added; the tx can be sent by the invitee (or relayer) without any existing member submitting.

Relevant code: `contracts/contracts/near/context-config/src/invitation.rs` (commit + reveal), and `mutate.rs` dispatching `CommitOpenInvitation` / `RevealOpenInvitation`.

---

## Current group flow and the mismatch

- **On-chain:** `add_group_members(signer_id, group_id, members)` is only reachable via `GroupRequest::AddMembers` in `mutate()`. The contract uses the **signer** of the request as the authorizer: `group.admins.contains(signer_id)`.
- **Core join handler:** In `join_group.rs`, the **joiner** builds a group client with **their own** signing key and calls `group_client.add_group_members(&[signer_id])`. So the transaction signer is the **joiner**, and the contract sees `signer_id == joiner`. The joiner is not an admin, so the contract would reject with "only group admins can add members".
- So with the current contract, the joiner **cannot** add themselves via `AddMembers`; only an admin can add members, and the admin would have to submit the tx (e.g. server/inviter submits on behalf of the joiner).

---

## Aligning group join with context invitation (so the joiner can add themselves)

To mirror the context-invitation pattern for groups:

1. **Add a two-step group invitation flow in the contract** (new module, e.g. `group_invitation.rs`, and new `RequestKind` variants or separate entrypoints if preferred):
   - **Commit:** `commit_group_invitation(group_id, commitment_hash, expiration_block_height)`  
     - No admin check; store commitment (e.g. in a new `group.commitments_open_invitations` or similar keyed by group_id).
   - **Reveal:** `reveal_group_invitation(payload: SignedGroupRevealPayload)`  
     - Payload contains: group_id, inviter identity, invitee identity, expiration, inviter signature over invitation, invitee signature over payload data.
     - Contract:
       - Verifies group exists, commitment exists and is consumed, not expired, invitee not already in `group.members`.
       - Verifies **inviter** signature over the invitation; assert **inviter is a group admin** (so the invitation is authorized).
       - Verifies **invitee** signature over payload (consent).
       - Marks inviter’s invitation signature as used (replay protection).
       - **Inserts invitee into `group.members`.**

2. **Core / SDK:**  
   - Add client methods and join flow that use **commit_group_invitation** + **reveal_group_invitation** (same pattern as `join_context_commit_invitation` / `join_context_reveal_invitation`).
   - In `join_group.rs`, stop calling `add_group_members` as the joiner; instead perform commit (if desired) + reveal so the **joiner** (or relayer) submits the reveal and the contract adds them.

3. **Keep** `add_group_members` for **admin-only** bulk adds (no invitation); use the new invitation flow when the joiner should be able to add themselves via an invitation payload.

This keeps group join consistent with context invitation: the joiner (or relayer) sends the reveal tx, and the contract adds the member based on inviter + invitee signatures and inviter admin check, without requiring an admin to submit the add.
