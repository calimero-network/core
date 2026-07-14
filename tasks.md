# Security review — status & plan

_Last updated 2026-07-02. Verified against `master` @ `fec752e3` + open PRs._

Legend: ✅ fixed (merged) · 🟦 fixed (open PR) · ⛔ won't-fix (decision) · ⬜ left

---

## A. Sync / handshake / DHT findings

| # | Sev | Finding | Status | Where |
|---|-----|---------|--------|-------|
| 1 | H | Inbound handshake identity not bound to transport (`sync/manager/mod.rs`) — `their_identity` taken from attacker `party_id`; no PoP before serving DAG heads/snapshot/deltas | 🟦 open PR | **#3167** |
| 2 | M | Namespace pre-registers identity via direct write (`namespace_sync.rs`) — `add_member` from replayable invitation + unverified `party_id` | 🟦 open PR | **#3167** (join `Init` PoP) |
| 3 | M | TEE admission nonce self-chosen / replayable (`tee_attestation_admission.rs`) | ⬜ left | — (owned by teammate) |
| 4 | M | DHT blob providers trusted without validation (`kad.rs`) — provider peer ids dispatched unvalidated (eclipse) | 🟦 open PR | **#3168** |

### #3167 — `fix/sync-init-pop-transport-binding`
- Adds `InitProof` on `StreamMessage::Init`: Ed25519 PoP over `(context_id, party_id, initiator PeerId)`, verified by the responder against the transport-authenticated PeerId before state-read + join paths. Join path also requires `party_id == joiner_public_key`.
- Exempt (by design): blob/group-key shares (ECDH-wrapped), namespace backfill (self-verifying signed deltas), entity pushes (per-action auth).
- Tests: InitProof verify/replay/forge/tamper + Init borsh round-trip; responder-gate classification. Full sync-sim (322) + `sync::` (243) green.
- Review: 3 MeroReviewer comments addressed (DIGEST_SIZE constant; freshness rationale in docs; gate-ordering note) — commit `264a3381`.
- **Left before merge:** live 2-node merobox smoke (state-read + namespace join) — sim harness bypasses `internal_handle_opened_stream`. Coordinated-upgrade wire change (Borsh-positional field on `Init`).

### #3168 — `fix/dht-signed-blob-provider-records`
- Adds signed `BlobProviderRecord` (public key + Ed25519 sig; verify requires sig valid **and** embedded key hashes to claimed peer). `announce_blob` signs with the node network keypair; read path + inbound-replication validator authenticate before dispatch/store.
- Tests: record roundtrip/wrong-key/tamper/peer≠key/garbage + kad validator paths. Network suite (110) green. LGTM from MeroReviewer, no inline comments.
- **Left before merge:** soft wire change (records TTL'd + re-announced, converges); blob discovery only interoperates between same-format peers.

---

## B. Server / blob / admin-API batch

Re-verified against `master` — **most were already fixed** by earlier hardening PRs (#3040, #3138, …). Kept here for the record.

| # | Sev | Finding | Status | Evidence on `master` |
|---|-----|---------|--------|----------------------|
| 5 | M | Blob responses cached public `max-age=3600` (`blob.rs:198`) | ✅ fixed | `Cache-Control: private, no-cache` (blob.rs ~219) |
| 6 | M | Fabricated zero metadata corrupts blob HTTP framing (`blob.rs:195,264`) | ✅ fixed | missing metadata now returns **500**, never fabricates `Content-Length: 0`/zero ETag (blob.rs ~283) |
| 7 | M | Blob delete unscoped IDOR (`blob.rs:322`) | ✅ fixed | `delete_handler` refuses blobs referenced as application artifacts (`is_blob_application_artifact`) |
| 8 | M | Blob upload announces to caller-supplied context w/o membership check (`blob.rs:128`) | ✅ fixed | upload announces only when node is a member; else "Skipping announce: not a member" |
| 9 | M | List endpoints don't clamp `limit` (`list_group_members`, `list_all_groups`, subgroups/contexts) | ✅ fixed | `.min(MAX_LIST_LIMIT)` on all list handlers (#3138) |
| 10 | M | `GET /contexts` no pagination + N+1 (`get_context_ids.rs`) | ✅ fixed | `offset.min(MAX_OFFSET)` + `limit.min(MAX_PAGE=1000)` |
| 11 | L | WS subscribe no membership check — IDOR (`ws/subscribe.rs`) | ✅ fixed | per-context `has_member` gate; unauthorized ids dropped |
| 12 | L | `invite_specialized_node` arbitrary `inviter_id`, no rate limit | ⛔ won't-fix | still verbatim `inviter_id` on master; the fix (PR **#3136**) was **closed** — specialized-node feature being dropped |
| 13 | L | Raw-error 500s / wrong codes; `get_context_storage` returns 0; alias validation | 🟨 mostly fixed | `get_application`/alias handlers use `parse_api_error`; `get_context_storage` implemented (`context_storage_bytes`). **Residual:** `create_alias` does no explicit handler-level length/charset/dup guard — it delegates to the typed `Alias` + `node_client.create_alias`; needs a check of whether the `Alias` newtype enforces charset/length on construction |

---

## C. Plan for what's left

1. **#3167 & #3168 → merge.** Both are open with green local tests. Before merge:
   - Run a live merobox 2-node smoke: cold namespace join, a state-read sync (DAG-heads/delta/snapshot), and a blob fetch — confirms the PoP gate and signed-provider path over a real transport (the sim harness does not exercise `internal_handle_opened_stream`).
   - Both are network-upgrade-sensitive; land behind the usual coordinated-rollout messaging (see PR bodies). #3167 is a hard flag-day for state-read/join sync; #3168 converges softly.

2. **Finding 3 (TEE nonce) — teammate.** Not started. Fix = verifier-issued challenge nonce, consumed once, bound to the announcer. Adjacent open work: #2996 (rate-limit/dedup on the verify entrypoints) mitigates spam but does **not** change the self-chosen-nonce model. Coordinate so the two don't collide in `tee_attestation_admission.rs`.

3. **Finding 12 (invite_specialized_node) — confirm won't-fix.** Left vulnerable on `master` because the specialized-node invite feature is being removed (PR #3136 closed on that basis). If the feature is NOT actually being deleted, this reverts to an open [L] and #3136 should be reopened/rebased. **Decision needed:** delete the endpoint, or reopen the fix.

4. **Finding 13 residual (alias validation) — small follow-up.** Verify whether `create_alias` enforces length/charset and rejects duplicates; if not, add validation returning `400` (one-file change, no wire impact). Low priority.

5. **Docs (both PRs).** AutoDocs bot flagged `architecture/crates/{node,sync,network}.html` + `wire-protocol.html` as stale. Handled by the AutoDocs pipeline on merge, not by hand — no action unless the bot's follow-up doc PR fails.
