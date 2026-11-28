# Handoff: finishing system-group tenant sharding (S5 remainder → S7 → S8)

This is an implementer's handoff for the work that remains after PR #5559 (the
`Orgs`-area flip, gated off). Read it with the design note
([`system-group-sharding.md`](./system-group-sharding.md)) — that note is the
*why*; this is the *what next, in what order, and the traps*. Everything here is
grounded in the code as it stands on the PR branch.

## 1. Where things stand (read this first)

- **`shard::ACTIVE = false`** (`crates/node/src/system/shard.rs`). The cutover
  switch is **off**: every command stays on the single system group
  (`SYSTEM_GROUP` = 1), so the committed behaviour is unchanged. Nothing below
  takes effect at runtime until `ACTIVE` flips (S8).
- **Only the `Orgs` area is flipped**: `shard::flipped(area)` returns true for
  `Area::Orgs` alone. Org membership, teams and invites route to and apply on
  the org's tenant group; everything else still applies on root.
- **The machinery is all built and behaviour-preserving**: the routing/ownership
  layer (`shard.rs`), tenant-group lifecycle (every system voter forms and hosts
  each org's tenant Raft group), role-aware `apply` (`shard::owns`), the
  `SeedTenant` identity keystone, resolve-at-propose baking, and the cross-shard
  read helpers. The `Orgs` flip exercises all of it end to end.

- **Read this before trusting the earlier measurements.** Re-running with the
  cutover on at the branch head, an organization could not gain a member
  (`SetOrgRole` refused `unknown account` on the tenant; invite redemption raced
  the root `invite_org` directory; prejudge judged `Orgs` commands on root), and
  every governance command judged its actor against root's stale copy of the
  organization. Those are fixed in the slice that follows this handoff (design
  note, "The governance areas stay on root: the split organization record"),
  with what remains open listed there. Re-validate any area at `ACTIVE = true`
  yourself (§6) rather than relying on a past green run.

**The remaining flips are not mechanical.** `shard::area_placement` marks
`Apps`/`Accounts`/`Recovery` as `Tenant`, but the S2 field partition (design
note, "Field classification") keeps the **account identity directory**
(`accounts`, `credentials`, `recovery_keys`, `account_recoveries`, `backup_keys`,
`activity`) on **root**, because an account is cluster-wide. So an area whose
commands write account-following state cannot simply be added to `flipped()` —
its machine would not own the state it writes. Resolving that split **is** the
work; see §4.

## 2. The toolkit (mechanisms an implementer uses)

All in `crates/node/src/`.

- **`system/shard.rs`** — `ShardId` (`Root` | `Tenant(OrgId)`), `Scope`
  (`Cluster` | `Org` | `OfApp` | `OfAccount`), `area_placement(Area)`,
  `flipped(Area)` (the per-area switch — **add an area here to flip it**),
  `shard_for(active, tenant)`, `owns(role, tenant, active)` (apply-side mirror),
  `tenant_of(scope, resolve_org)`, `TENANT_GROUP_BASE` (`1<<32`), `ACTIVE`.
- **`SystemCmd::scope()`** (`system.rs`) — exhaustive classifier mirroring
  `SystemCmd::area`. For `OfApp`/`OfAccount` the owning org is read from the root
  directory at propose time. **A new command must be classified here.**
- **Role-aware apply** — `shard::owns(role, tenant, active)` guards that a
  command is applied by exactly the shard its write routed to. A command on the
  wrong role is a routing bug, not a no-op.
- **Resolve-at-propose (baking)** — a command that reads another shard's state
  bakes those facts into the entry at propose, so `apply` reads only its own
  shard (the determinism rule, `crates/node/CLAUDE.md` rule 1). Examples already
  in the tree: `CreateApp`/`TransferApp`/`SetAccess`/`SetAppMetadata` bake
  `org: Option<Box<Org>>`; `OfferApp` bakes `serving: Vec<Org>`; `RedeemInvite`
  bakes `org`/`redeemer`. Owner-membership facts bake as boolean flags
  (`owner_in_org`, `owner_in_team`, `in_deployment`, …).
- **The cross-shard read convention** (documented at the hub method in
  `system.rs`, above `member_of`): a **plain** method reads this shard's own
  state; a **`_with`** twin takes an `extra: &BTreeMap<OrgId, Org>` of orgs
  resolved from their tenant shards (multi-org reads); an **`_at`** twin takes a
  single `Option<&Org>`; a **`Node` `*_fresh`/`*_view` helper** (`node.rs`) fans
  the orgs out and fills the map before proposing or answering. Gates on a
  sharded read go through the `Node` helper; local reads call the plain method.
- **`SeedTenant`** (`system/orgs.rs`) — once a tenant group forms, the leader
  seeds it with the org record and a projection of its members' passkeys, so the
  tenant verifies consent **from its own log alone** (`Proofs::consent_of`).
  This is the identity keystone that lets a tenant apply a consent-bearing
  command deterministically without trusting a peer. Any new tenant-applied
  command that checks a consent proof depends on the needed credential being in
  the tenant's seed (see §4, the credential-binding decision).
- **`Node` helpers** (`node.rs`): `sys_with_org(org, f)` hands a gate the root
  state + the org from its tenant together; `org_of(org)` fetches one org from
  its shard; `sys_org(org, f)` reads an org's own-data shard; `system_shard`
  resolves a `Tenant(org)` to the hosted group; `ensure_tenant_group` + the
  reconcile loop (around `node.rs:3000`) form/host/seed tenant groups;
  `release_org_view`/`deployment_serve_view`/`*_fresh` build the `extra` maps.

## 3. The recipe to flip one area (from the `Orgs` flip)

For each area X you flip (add `Area::X` to `flipped()`):

1. **Confirm the state is tenant-owned.** Every field the area's commands write
   must be a tenant-owned field (S2 table). If any is root (account-following),
   stop — resolve the split first (§4).
2. **Scope every command.** `SystemCmd::scope()` must return the owning org for
   each of the area's variants. Commands that name only an app/account resolve
   through `tenant_of`'s `OfApp`/`OfAccount` lookup; token-keyed commands that
   name no org need a root directory (as invites got `invite_org`/`BindInvite`).
3. **Bake cross-shard reads.** For each thing the area's `apply` reads that lives
   on another shard, add a baked field to the command and read it at propose
   (the resolve-at-propose pattern). Apply judges the baked value.
4. **Route every reader.** Every gate/API/consent-page path that reads the area's
   state must read it from the owning shard (`sys_with_org`/`org_of`/`sys_org`,
   or a `_with`/`_fresh` fan-out for multi-org reads).
5. **Validate at `ACTIVE=true`** (see §6): the area's `it` tests must pass with
   the cutover flipped on, run isolated.

The `Orgs` flip touched ~128 org reads across ~20 files; expect each area to be a
comparable, self-contained pass.

## 4. The central unresolved design problem: identity on root, scope on tenant

This blocks `Accounts` and `Recovery` and is the real research the design note
flags. **Decide and document it before implementing either.**

- **An account is cluster-wide.** `accounts`, `credentials`, `recovery_keys`,
  `account_recoveries`, `backup_keys`, `activity` stay on **root** so one account
  can act in many orgs. A tenant command whose authority is a consent proof must
  verify it against the actor's credential — which is on root, not the tenant.
- **The resolution the note assumes (fork Y, attestation-level):** project the
  credential a tenant needs into its seed (`SeedTenant` already projects member
  passkeys), kept in sync on credential/membership change, so the tenant verifies
  from its log. The open risk (design note, "Open questions") is a faulty or
  compromised leader baking a forged credential; the note's answer is the
  attestation boundary the fleet already trusts (the leader is attested; consent
  is re-verified from the baked passkey). **If you want a stronger guarantee,
  add a signed credential binding** (operator- or cluster-key signed at
  registration, verifiable by any shard) carried in the entry. Pick one and
  write it into the design note before flipping.

### `Recovery` (`system/recovery.rs`, `Area::Recovery`)
Commands: `SetAccountRecoveryKey`, `RemoveAccountRecoveryKey`,
`StartAccountRecovery`, `CancelAccountRecovery`, `FinishAccountRecovery`,
`SetBackupKey`, `ImportHolder`, plus the org-scoped `SetRecoveryKey`/
`StartRecovery`/`CancelRecovery`. **Split required:** the `*AccountRecovery*` and
`backup_key`/`ImportHolder` commands touch root account state → keep on root; the
org-scoped recovery commands → tenant. This likely means `Recovery` does **not**
go into `flipped()` wholesale; instead route by command (as the `Scoped` areas
do, below), or split the area. Decide which.

### `Accounts` (`system/accounts.rs`, `Area::Accounts`) — the hardest, biggest payoff
Sessions/refreshes/API keys are the largest write source — the whole point of
sharding. Design note "The crux: identity is root, sessions are tenant": a
session shards by the **org context it is minted for**; account-level sessions
(no org) stay on root. Work:
- Give the sign-in contract an org the token names; a principal acting in several
  orgs holds a session per org.
- Move `refresh`, `rotated_refresh`, `revoked_sessions`, `revoked_at`,
  `api_keys` to the tenant (by org context); keep `accounts`/`credentials` on
  root.
- **The access gate (S6) becomes a two-shard read**: account identity from root,
  the session/membership from the acting org's tenant, each at a read index under
  O2's lease (`Config::sys_read_lease`). `api::gate`/`principal` must carry the
  request's org context.
This is a subsystem change; do it last among the flips and on its own.

### `Approvals` / `Deployments` / `Updates` (`Placement::Scoped`)
Designed to split by command scope (org-scoped → tenant, cluster-wide → root),
but **the audit found they cannot move before `Apps`**: their state is read by
root-applied `Apps` commands (`UpgradeApp` reads update policies, first
approvals and deployments) and by root's Raft-admission index (release
approvals), and their own commands read root (`apps`, `deployers`,
`governance`). See the design note, "The governance areas stay on root: the split
organization record". What landed instead keeps them on root and correct with
the cutover on: readers merge the organization record (`Org::with_root_fields`
in `Node::org_of`), and each governance command carries the actor's projected
standing (`Org::standing_of`, `Node::standing`/`deployment_standings`) that
`apply` judges membership against. The open items in the same class are listed
there; close the root-applied `Apps` membership checks (same standing pattern)
before or with the `Apps` flip.

### `Apps` (`Area::Apps`)
App records belong to an org (`CreateApp` already bakes the org). The app
directory is read widely (gates, reports, consent page), so flipping it is mostly
reader-routing work (step 4) plus baking the few cross-shard apply reads. No
account-identity split, but high read breadth.

**Suggested order (revised after the governance audit):** close the remaining
root-applied `Apps` membership checks with standings → `Apps` together with the
scoped governance areas (`Deployments`, `Updates`, `Approvals`), whose state
`Apps` reads → `Recovery` (split) → `Accounts` (the crux).

## 5. Pitfalls this session hit (save yourself the round-trips)

- **Determinism (CLAUDE.md rule 1).** `apply` reads only the entry, its own
  shard's state, and the logged cluster version — never another shard. Any
  cross-shard fact must be baked. A `_with`/`_fresh` fan-out is for
  propose/gate/read time, **never** inside `apply`.
- **`check-no-user-data-at-info.py`** (a trust-boundary CI gate, not in clippy):
  no `info!`/`warn!`/`error!` may log an error's words or user text. Route errors
  through `crate::log_safe::kind(e.as_ref())` / `log_safe::serde(&e)` and log
  `error_kind = %…`. Org ids are fine; error text and names are not.
- **Rustdoc intra-doc links** (`Rust docs` CI job, `-D warnings`, **no**
  `--document-private-items`): a public doc may not link a `pub(crate)`/private
  item. Make links to `shard::ACTIVE`, `shard::TENANT_GROUP_BASE`,
  `Node::system_shard`, etc. plain code spans, not `[` `]` links. Verify with
  `RUSTDOCFLAGS="-D warnings" cargo doc -p calimero-node --no-deps` (do **not**
  add `--document-private-items`; it hides the error).
- **Trust boundary PR block.** Any PR touching `TRUST_BOUNDARY_PATHS`
  (`scripts/check-trust-boundary.py`) must carry the template's Trust boundary
  checklist in its body, every line ticked, or the `Trust boundary` job fails.
- **Merging `master`.** `master` has refactored the org-authority helpers into
  `check_delete`/`check_access`/`check_app_metadata`/`prejudge` and added
  `hidden_from`/`origin_admits`, and changed the arity of `manages_app`/
  `may_reach`/`owner_acts_as_logged`/`is_org_admin`/`may_set_access`/
  `check_grants`. When you merge, **keep the sharding's org threading** (don't let
  master's internal-read refactor silently revert the cross-shard routing) and
  reconcile the helper signatures.
- **Disk.** The `it` test binary is ~1.7 GB; a dev box with a small write
  allowance can't link it. Keep `target/debug/incremental`, `build`, `doc` and
  stale `deps/it-*.tmp*` pruned. CI has the headroom; lean on it for the full
  suite.

## 6. Validation

- **Fast, local:** `cargo build -p calimero-node`,
  `cargo clippy -p calimero-node --all-targets -- -D warnings`,
  `cargo fmt -p calimero-node -- --check`, `python3 scripts/check-code-refs.py`,
  `python3 scripts/check-no-user-data-at-info.py`,
  `RUSTDOCFLAGS="-D warnings" cargo doc -p calimero-node --no-deps`.
- **The flip itself only shows at `ACTIVE=true`.** CI runs `ACTIVE=false`, so it
  does **not** validate a flip. To validate: set `shard::ACTIVE = true`, build
  the `it` binary, run the area's tests isolated (`--test-threads 1` — the
  per-org tenant groups make 4-way parallelism tip timing-bounded tests on load),
  confirm green, then **reset `ACTIVE=false` before committing**. The whole suite
  passed isolated at `ACTIVE=true` for the `Orgs` slice this way.
- **Real apps e2e** (strong, fits a constrained box): build `merod`+`meroctl`
  from the branch, `cd` the `calimero-network/apps` repo, set
  `MEROD_BIN=…/target/debug/merod` and `meroctl` on `PATH`, `export
  CARGO_TARGET_DIR` to a shared dir, then `pnpm merod-local e2e <app>` per app
  (kv-store, conformance, scaffolding-e2e, mero-calendar, mero-chat, mero-docs,
  mero-vote all passed on the `Orgs` slice). This exercises org-scoped access,
  members and private-release visibility against the branch's node.
- **The real gate (not available in a dev sandbox):** failover, cross-zone
  placement and split-brain safety of tenant groups need real `merod` processes
  (`crates/node/tests/fault/`) and a multi-node deployment. The design note is
  explicit this is the gate between the plan and production — **the cutover (S8)
  must not land without it.**

## 7. S7 — per-shard snapshots, recovery, backup

- `SNAPSHOT_FORMAT` is `crates/log/src/machine.rs:164` (currently `2`). Tagging a
  snapshot with its shard raises it; **keep a reader for the old shape and add a
  test that reads data written in the old format** (CLAUDE.md rule 4,
  compatibility doc) — or declare it part of the hard cutover.
- Make backup, recovery (`node/restore.rs`, `recovery_disk.rs`, `backup.rs`) and
  `merod inspect` iterate **every** system shard (root + each tenant), not only
  `SYSTEM_GROUP`. Grep `SYSTEM_GROUP` across the node crate and replace the
  single-group assumptions with per-shard or root-only forms (`member_addr`,
  `status`, `subscribe_applied`, metrics).

## 8. S8 — multi-tenant tests, flip on, delete the old path (the cutover)

1. **Multi-tenant `it` tests** (`tests/it/`, near `instance_orgs.rs` and the
   existing `an_orgs_tenant_group_is_seeded_with_its_record_and_members`): two
   orgs' state lands in different tenant groups led by different nodes; a
   cross-org operation is root-coordinated; failover of a tenant group keeps its
   org's control plane; a cross-shard access read is fresh under the lease. Add a
   multi-tenant **mode to `cluster_throughput`** so the per-tenant throughput win
   is actually measured (today it reads unchanged because the hot paths are
   unflipped).
2. **Flip `ACTIVE = true`** as the default, and **delete the single-group path**
   (`shard_for`/`owns` collapse to always-tenant; remove the `active` parameter).
   Fix the existing suite, much of which assumes one system group.
3. **Declare the hard cutover** (CLAUDE.md "Definition of Done", compatibility
   doc): `!` in the release PR title, a `[[cutover]]` entry in
   `crates/node/api-deprecations.toml`, the release in `.github/upgrade-from` set
   to `hard-cutover:`, and mdma's `release_compat` `CUTOVERS` line. Add an
   `upgrade_cases/` case (the dir has `20_accounts.py`, `30_organizations.py`, …)
   and a `DeepCase` for the multi-tenant behaviour.
4. **Do not merge without the multi-node validation in §6.**

## 9. Definition of done for each slice (so the PR stays landable)

Each slice is its own commit that compiles, is behaviour-preserving while
`ACTIVE=false`, passes the §6 fast-local gates, and — for an area flip — is shown
green at `ACTIVE=true` isolated + the app e2e. Keep the design note and this
handoff updated as decisions are made (the identity-binding choice especially).
The cutover slice is the only one that changes shipped behaviour; it carries the
declaration in §8 and waits on real-cluster validation and a human's go.
