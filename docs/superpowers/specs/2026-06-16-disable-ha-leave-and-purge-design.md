# Disable HA → leave namespace + all subgroups + delete data & keys — Design

| | |
|---|---|
| **Goal** | When HA is disabled for a namespace, the TEE fleet node leaves the namespace **and every subgroup**, and deletes its local data **and all keys** (signing keys + AES group encryption keys) for that namespace subtree. |
| **Source of truth** | **mdma.** The node leaves + purges whenever mdma definitively drops the namespace from its fleet assignments — for *any* reason (HA disabled, slot reclaimed, MRTD distrust). |
| **Date** | 2026-06-16 |
| **Branch (part 1)** | `feat/self-purge-delete-group-keys` (core) |
| **Relation** | Teardown side of the namespace-TEE program; resolves proposal §11 Q6 (disable cascade) and the §6 disable-amplifier. Pairs with the merged self-purge lineage (#2686/#2721/#2724/#2725/#2764) and Phase 1 admission (PR #2772). |

---

## Problem

Disabling HA in mdma (`HaRequest.status=disabled`) is a **soft** signal: the namespace drops out of `should-join`, the sidecar stops being assigned it, and mdma's cap accounting ages out after the freshness TTL (900s). **Nothing evicts the TEE node at the governance layer, and nothing deletes its keys.** Verified against source:

1. **No trigger.** Core has no mdma client and no disable hook; mdma never writes the governance DAG. So `TeeMemberRemoved` is never emitted on disable, `self_purge` never runs. The node's `ReadOnlyTee` rows at the root and in every Restricted subgroup — and the delivered subgroup keys — **persist on-node indefinitely**.
2. **Even an explicit leave doesn't delete the encryption keys.** `meroctl namespace leave <ns>` → `GroupOp::MemberLeft` cascade → `self_purge` *does* evict from the namespace + all subgroups and deletes membership rows, **signing** keys (`GroupSigningKey`), namespace identity, contexts, gov-op log, and unsubscribes. **But it does not delete the AES group *encryption* keys** (`GroupKeyEntry` / `GroupKeyring`). No code path in core deletes group encryption keys at all — `GroupKeyring` has no delete method. So the replica keeps the decryption keys for everything it ever held, at root and every subgroup.

Net: turning HA "off" neither evicts the replica nor takes the keys away. This is the forward-secrecy / cleanup hole this design closes.

---

## Design

Three parts, in dependency order. Part 1 (core) must land **and be released** before Part 2 (sidecar) is meaningful — otherwise a triggered leave evicts but still leaves the AES keys on disk.

### Part 1 — Core: make purge actually delete the group encryption keys

The eviction + cascade machinery already exists and is correct (`MemberLeft` → per-subgroup removal + `self_purge` `PurgeAction::Namespace`, which already iterates root + all descendants via `cascade_namespace_state`). The single gap is that `delete_group_local_rows` does not delete `GroupKeyEntry` rows.

- Add `GroupKeyring::delete_all_for_group(&self) -> EyreResult<()>` (mirrors `SigningKeysRepository::delete_all_for_group`) that deletes all `GroupKeyEntry` rows for its `group_id`.
- Call it inside `delete_group_local_rows` (`crates/governance-store/src/local_state.rs`), alongside the existing `SigningKeysRepository::delete_all_for_group`, so every group the purge sweeps (root + each subgroup) has its AES keys deleted too.
- Verify the namespace purge also removes the **context state / application data** for the subtree; if `cascade_namespace_state` leaves encrypted state rows, deleting the keys crypto-shreds them (acceptable), but prefer deleting the state rows where a clean delete exists. Document whichever is the case.
- This is a **purge-contract change** in `calimero-governance-store` (sensitive; same area as the merged self-purge work). It is independently valuable: `self_purge` should delete group keys regardless of this feature. Ship as its own core PR.
- **Tests:** unit test that `delete_group_local_rows` removes `GroupKeyEntry` for a group; extend the self-purge namespace-cascade test to assert no `GroupKeyEntry` survives for the root or any subgroup after `PurgeAction::Namespace`. Reuse the existing self-purge test fixtures.

**Non-goal for Part 1:** the trigger. Part 1 only makes "leave/purge" complete (keys deleted). It does not decide *when* to leave.

### Part 2 — mero-tee: sidecar leave-on-disable, safety-gated

The trigger lives in the fleet sidecar (`mero-tee/mero-tee/ansible/roles/merotee/templates/fleet-sidecar.sh.j2`), the only local agent that sees mdma and can drive `meroctl`.

- **Poll-success gate (safety-critical).** Today `poll_mdma` swallows any curl failure into `{"assignments":[]}` — so a transient mdma outage looks identical to "everything disabled." Add an explicit success signal (mirror `confirm_assignment`'s `0/1` return): the leave step runs **only** on a should-join that returned **HTTP 200 with a parseable assignments body**. On any failure, do nothing (the existing benign re-join behavior is unchanged).
- **Leave on definitive drop.** On a good response, compute `to_leave = confirmed − desired`. For each, run `meroctl --home … --node … namespace leave <group_id>` (the `group_id` is the namespace id, hex, passed verbatim from the should-join response — same value used for `fleet-join`). Make it **idempotent / non-fatal**: a leave of a namespace the node already left returns an error string ("nothing to leave" / "not a direct member") — log and continue, never abort the loop. Then let the existing intersection-prune persist the dropped set.
- **Insertion point:** in the `while` loop, after the join-reconcile and before the existing prune, gated on poll-success.
- **Release:** bump `mero-tee/versions.json` `imageVersion`; rebuild → new MRTD → publish `published-mrtds.json` + update `compatibility-catalog.json`.

### Part 3 — mdma: release coordination

- mdma must add the new node-image MRTD to its allowlist (consumed from `mero-tee-vX.Y.Z/published-mrtds.json`) before the new image rolls out, else new-image nodes get zero assignments.
- No mdma logic change is required for the feature itself — disable already works (soft status flip). mdma stays the source of truth; the node reacts to mdma's `should-join` response.

---

## Safety & correctness

- **Only a definitive mdma signal triggers leave.** A 200-OK should-join that omits a previously-confirmed namespace is mdma authoritatively saying "you are not assigned here" — whether due to disable, slot-reclaim, or MRTD-distrust. In all cases the node is no longer entitled, so leave + purge is correct. Transient failures (curl error/timeout/non-200/unparseable) never trigger leave.
- **No false-purge churn.** A healthy node polls every ~1s, so its FleetAssignment never ages out (900s TTL) and is never spuriously reclaimed; it only drops on a real disable/reclaim.
- **Idempotent + crash-safe.** `meroctl namespace leave` is a safe error on a non-member; core's self-purge has a startup reconcile sweep that completes an interrupted purge on restart.
- **Cascade is symmetric.** `MemberLeft` removes the node's direct row in every subgroup it held one in and emits per-subgroup `TeeMemberRemoved`; `self_purge` `PurgeAction::Namespace` purges root + all descendants. After Part 1, that purge includes the AES group keys.

## Out of scope

- Node-resident liveness heartbeat / verifier-side revocation reaper (dropped — see memory; no driver).
- Changing mdma's pull/soft-disable model (it stays source of truth).
- Open-subgroup handling — covered transitively (Open subgroups inherit the namespace; purging the namespace subtree removes them too).

## Sequencing

1. **Part 1 (core)** — key-deletion in purge. Land + release (merod build).
2. **Part 2 (mero-tee)** — sidecar leave-on-disable, gated. New node image (picks up the released merod + the new sidecar). New MRTD.
3. **Part 3 (mdma)** — trust the new MRTD before rollout.
