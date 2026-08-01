# Handoff: finish stage 2/3 of account-keyed writer sets (PR #3344)

Worktree: `/Users/chefsale/workspace/calimero/core-wt-accounts`

## Where to pick up

```bash
git checkout feat/account-keyed-writers   # PR #3344
git log --oneline -2                      # 8a51ac29c (stage 2 wip) on 95a3f54e9 (stage 1)
cargo check -p calimero-storage           # 0 errors — the LIB compiles
cargo check -p calimero-storage --all-targets   # ~58 TEST errors — this is the next job
```

**State of the branch, read this before pushing anything.** `origin` is at `95a3f54e9`
(stage 1 only, green). Local is one commit ahead with the stage-2 lib flip, which is
**deliberately unpushed**: the lib compiles but the test targets do not, so pushing now
turns #3344's CI red. Push once step 1 below is done. `wip/account-keyed-writers-stage2`
points at the same commit as a safety net.

Read `git log -1 wip/account-keyed-writers-stage2` first — it documents what is done and why.

## What this is fixing

#3320 (merged, squash `fcc74a1b7`) gave the system accounts: one person, many devices. But
storage's own access-control primitives still resolve `env::device_id()`, so to storage a
person's two devices are **two unrelated strangers** — grant your laptop admin and your
phone is refused. That is the problem the account plane exists to remove, resurfacing one
layer below governance. It is also the one review thread left open on #3320.

## The distinction everything rests on

**Gates take the account. Stamps keep the device.**

- **Gate** = "may this person write?" → `env::account_id()`. Eleven sites, all done.
- **Stamp** = "who wrote this?" → `env::device_id()`, unchanged. `SignatureData.signer`
  names whatever will actually verify, and only a device holds a signing key. `owner_of`
  on `AuthoredMap`/`AuthoredVector` is per-writer state.

Three sites do both in one breath — authorize by account, then stamp the key that will
sign. Read those before changing anything.

**Do not run a blanket `PublicKey -> AccountId` replace.** I tried; it compiles and is
wrong. Both types are 32 bytes so nothing fails to build, but it silently moves
`owner_of`'s stamps onto accounts — and two devices of one account then share a counter
slot and an HLC seed and lose each other's writes. The boundary is per-FILE:
`shared.rs`/`access_control.rs`/`permissioned.rs` are entirely principals; `user.rs` and
`authored_*.rs` are entirely stamps.

## The design decision, already settled — do not re-litigate

The writer set names accounts; a signature names a key. The bridge is
`ApplyContext.signer_account`, and **the receiver resolves it at the delta's causal cut.**

- **Not carried on the delta.** I built that first and it is a hole: a sender-supplied
  principal is self-asserted, so a peer names a writer's account beside its own signing
  key and the signature still verifies — it proves possession of a key, nothing about
  whose key it is. Reverted.
- **Not resolved from live state.** Two nodes at different fold depths would disagree
  about who may write, which splits `scope_root` instead of rejecting a write.
- **`None` is a hard reject**, never a fallback to the locally executing account.

Consequence: `StorageDelta` is untouched, so **stage 2 is not a flag day** and does not
need bundling with #3346.

## Remaining work, in dependency order

1. **Test errors** in `crates/storage`. Each principal now needs a key to sign with AND
   an account granted. Mechanical, but see the testing section — do not just make them
   compile.

   **In progress (commit `83c1cbf8d`, local, unpushed.)** Helper layer added in
   `tests/common.rs`: `account_of_key` (the account a device key speaks for — derived
   from a DIFFERENT domain than the key bytes on purpose, so type confusion between two
   32-byte ids fails a test instead of passing it), `test_account` (account by seed, for
   one account holding two devices), `writers_of`, `apply_ctx_for`. Cleared: `action.rs`,
   `delta.rs`, `entities.rs`, `unordered_map.rs`, `unordered_set.rs`, `shared.rs`,
   `permissioned.rs`. Remaining, ~68 errors and all needing a per-site decision because
   signing and authorization meet in them: `tests/interface.rs` (31),
   `tests/write_hook_stale_writers.rs` (15), `rotation_log.rs` (9), `tests/index.rs` (7),
   `tests/sync_batch_resilience.rs` (5), `tests/sorted_index_convergence.rs` (1). Count
   rising as files clear is expected, not backsliding.
2. **node**: `rotation_log_reader::writers_at` -> accounts, and populate
   `ApplyContext.signer_account` on receive, resolved at the delta's cut. This is the
   real remaining engineering.
3. **apps**: `SharedStorage::new` / `AccessControl::new` take accounts.
4. **e2e** (stage 3): the two-device-admin scenario.

## Tests — the part that matters most

The existing suite passes with the gates device-keyed, which means **it cannot tell the
difference between the bug and the fix.** That is the gap to close. Every test below must
be verified to FAIL against pre-stage-2 code — if it passes both ways it is proving
nothing, and that has already happened twice in this feature.

**The headline behaviour, currently untested anywhere:**

- Grant account A (holding devices D1 and D2). Write from **D1** → accepted. Write from
  **D2** → accepted, *without D2 ever being granted*. This is the whole point, and today
  it fails.
- A device of an account that is NOT in the writer set → refused.

**The invariant that makes this safe, and the one a careless fix breaks:**

- D1 and D2 of the same account write **concurrently**. Assert their owner stamps stay
  DISTINCT and neither write is lost. If someone moves stamps onto accounts, this is the
  test that catches it — nothing else will, because it still compiles.
- Counter/HLC: two devices of one account must not share a slot or a seed.

**Revocation, which is where account-authorization and device-revocation meet:**

- Revoke D2 while account A is still a writer. D2's write must be refused — the account
  is authorized but the device is not bound any more, so resolution yields `None`.
  Verify it is refused for that reason and not incidentally.
- A pre-revocation write from D2 that arrives after the revocation still applies
  (causal-honour), matching the rest of the plane.

**Divergence, the failure class this whole feature spent its review on:**

- Two receivers folded to DIFFERENT depths receive the same delta. The one that cannot
  resolve the signer must DEFER, not reject. Assert both eventually reach the same root.
  A test that only checks "rejected" would pass while the split-brain remains.

**`resolve_signer` unit tests** (four cases, all cheap): valid signature + account in
writers → `Some(account)`; `signer_account: None` → `None`; account not in writers →
`None`; signature does not verify → `None`. Also assert the ordering property — a
non-writer costs no `ed25519_verify` (that was stage 1's security fix; a regression here
restores verification amplification).

**Stage 3 e2e** (`apps/scaffolding-e2e/workflows/`): grant device 1, have device 2 write,
assert it converges on both nodes. Add a barrier before every cross-node read or
`scripts/lint-e2e-convergence.py` (PR #3348) will flag it — and three scenarios failed
that way in one afternoon, so take it seriously.

## Repo gotchas that will bite

- **CI's clippy is stricter than a bare run.** Use exactly:
  `cargo clippy --workspace --all-targets --features calimero-storage/testing -- -D warnings`
- **A pre-commit hook runs `cargo fmt --check`** and will reject the commit. Run
  `cargo fmt --all` BEFORE committing, and never chain a `git reset --hard` after a commit
  in one command — a rejected commit plus a reset discards the work (this cost me a redo).
- **No backwards compatibility** is required (alpha), confirmed by Sandi.
- **Merge ruleset**: 1 approval + all threads resolved + signed commits + Rust/Lint/Build
  green; squash-only.
- Error count RISES mid-flip as each fixed type exposes the next signature. That is
  expected, not backsliding.

## Related work in flight

- **#3348** — merobox convergence linter, **MERGED** (`604cc17af`) and enforced in CI as a
  ratchet. Your stage-3 e2e will be checked by it: every cross-node read needs a
  `wait_for_sync` barrier covering the nodes involved. Run it locally before pushing:
  `python3 scripts/lint-e2e-convergence.py`
- **#3346** — retire `legacy_account_id`/`legacy_authorship`; bundle with P5/P6 for one
  flag day.
- **#3347** — join-via-inheritance fails permanently on a transport error to the only key
  holder. Unrelated but a live CI flake source.
- **`effective_writers` — CHECKED, not a bug** (#3349, closed by #3356, merged). A sender
  cannot supply it at all: the gossip path (`decrypt_delta_actions`) accepts only
  `StorageDelta::Actions` and refuses `CausalActions` outright, and the sync path's wire
  type (`CausalDelta`) has no field for a writer set. The applying node builds the
  envelope itself in `ContextStorageApplier::apply`, resolving from its OWN rotation log
  at `delta.parents` via `writers_at_authenticated` (each rotation entry must be signed
  by a prior-set ADMIN). So `CausalActions` is a host→guest envelope, not a peer→peer one
  — **this is the pattern step 2 should copy for `signer_account`**: resolve receiver-side
  at the cut, authenticated, and defer rather than accept when it cannot resolve.
