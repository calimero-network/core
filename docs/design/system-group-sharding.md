# Design: system-group tenant sharding (O5)

Status: **design + seam** (draft). This note specifies how the single
control-plane group splits by tenant, and ships alongside the first code seam
(`crates/node/src/system/shard.rs`). It changes no behaviour on its own; the
migration it describes is a hard, versioned change that lands behind a feature
gate and a declared cutover.

This is the O5 item of the performance design. Read it with the system-group
chapter (`docs/src/content/docs/protocol/system-group.mdx`) and the upgrades
chapter (`docs/src/content/docs/protocol/upgrades.mdx`); the terms here are
theirs.

## The problem

The control plane is a single Raft group of exactly three voters
(`SYSTEM_GROUP` = group 1, `membership::SYSTEM_VOTERS`), an `IN_MEMORY` machine
checkpointed through snapshots. It holds every area of control-plane state (11
`Area`s, `SystemCmd::area`) and, until the read-index lease (O2), is also on the
data-plane hot path: every credentialed or gated request reads it through a
read-index round (`Node::sys_fresh`).

Two pressures converge on that one group at fleet scale (hundreds to thousands
of nodes):

1. **Access reads** — one read-index round per credentialed request. O2
   (`Config::sys_read_lease`) takes these off the group with a bounded-freshness
   lease, which is the first and cheapest step of this ladder and is already
   landed separately.
2. **Control-plane writes** — sign-ups, sessions and refreshes, app and
   organization creation, approvals. These are real log appends on one leader,
   and one three-voter group's append rate is the ceiling. No lease helps a
   write; the only way past it is to **shard the writes**.

This note is about (2).

## Why shard by tenant, not by area

The state is already a clean partition into 11 areas (`SystemCmd::area`), so
splitting the log along area boundaries (one group per area, or a few groups of
grouped areas) looks natural. It is the wrong seam.

A single `CreateApp` is not confined to the `Apps` area. It reads:

- the **fleet** quota (`state.quotas.max_apps_per_account`, `Area::Fleet`),
- the caller's **organization** membership (`Area::Orgs`), and
- the **release** it instantiates (`Area::Packages`).

Cutting between areas shears this one command across three groups, turning a
single-group write into a distributed transaction. Many control-plane writes are
coupled this way: the hot, coupled core is *a tenant's* apps, members and
sessions reading *that tenant's* quotas and memberships.

Sharding by **tenant** (organization) keeps the coupled core whole inside one
shard and cuts only where the data is genuinely independent — one organization's
control plane from another's. The cross-shard reads that remain are to
**cluster-wide** state (the fleet quota, the package release), which changes
rarely and can be read across shards or cached, not written transactionally.

So the split is: a **root** group for cluster-wide state, and one **tenant
shard** per organization for that organization's own state. Leaders then spread
across nodes per tenant instead of funnelling through one.

## Placement of the 11 areas

`crates/node/src/system/shard.rs` encodes this as `area_placement(Area) ->
Placement` (`Root`, `Tenant`, or `Scoped`), with `shard_for(area, scope)`
resolving a specific command to a `ShardId`:

| Area | Placement | Why |
| --- | --- | --- |
| `Fleet` | Root | Nodes, quotas, pools, genesis, the cluster version — cluster-wide. |
| `Packages` | Root | Bundles, releases, publishers — shared by every tenant. |
| `Compute` | Root | The compute queue and byte charges — a cluster resource. |
| `SignUp` | Root | The sign-up policy and invites — a cluster policy. |
| `Apps` | Tenant | An app instance belongs to one organization. |
| `Orgs` | Tenant | An organization's members, teams and invites. |
| `Accounts` | Tenant | Sessions, refreshes and API keys dominate the write rate and belong to the account's organization context (see below). |
| `Recovery` | Tenant | Account and organization recovery, scoped to the organization. |
| `Approvals` | Scoped | An org-scoped approval lives in that tenant's shard; a cluster-wide one in the root. |
| `Deployments` | Scoped | A deployer grant within an org is tenant; the cluster owner and global policy are root. |
| `Updates` | Scoped | An org's update policy is tenant; the cluster update policy is root. |

`Accounts` is the subtle one. An account is a **cluster-wide identity**, but the
write-heavy part — sessions, refreshes, API keys — is most of the control
plane's write rate, and each belongs to the account's organization context, so
the area places as `Tenant`. The identity records a tenant needs (does this
account exist; is this passkey registered) are read across shards or cached, the
same cross-shard read `CreateApp` already needs for the fleet quota and the
release. This coupling, not the area boundary, is the whole argument for tenant
sharding.

### Step D: sessions

Sessions are the largest single write source, and a login that must append to a
global log is exactly the bottleneck tenant sharding removes. Two complementary
moves, both compatible with O2:

- **Fold session and refresh writes into the tenant shard** so a login writes
  only to one organization's group, not a global one.
- **Make revocation a leased check** (O2's model): a session is judged against
  the tenant shard's state within a freshness bound, so a login needs no global
  round. The revocation-latency bound is the same governance trade O2 documents.

## Cross-shard invariants

The migration is only sound if these hold across the root/tenant split. They are
the acceptance criteria for the real implementation:

1. **A tenant read of root state is at least as fresh as the rule requires.** The
   fleet quota and a release are read from the root group (or a cached copy with
   a bounded staleness that governance accepts), never a stale tenant-local
   guess. This reuses O2's read-index machinery per group.
2. **A command that writes one shard and depends on another's state judges that
   dependency at a read index**, so two nodes applying the same tenant log reach
   the same decision — the determinism rule (`crates/node/CLAUDE.md`, "Apply
   reads only the log") must still hold **within** each shard's log.
3. **Identity is global, authority is per tenant.** An account exists
   cluster-wide (root or a replicated index); whether it may act in an
   organization is the tenant shard's state. Authority is still judged twice
   (API before the ceremony, `apply` on every replica), now in the owning shard.
4. **Cross-tenant operations have a home.** Moving an app between organizations,
   or a cluster-wide approval, is a root-group operation or a two-phase operation
   the root coordinates; it never writes two tenant logs without a coordinator.

## Membership and placement

Each tenant shard is its own Raft group: three voters grown across failure zones
exactly as the system group does today (`grow_voters`, `membership::SYSTEM_VOTERS`).
The root group stays three voters. Placement of a tenant shard's voters reuses
the app-placement machinery (`placement.rs`) so a tenant's leader need not sit on
the same node as another's — the point of the exercise. O1 (the confirm window
off the heartbeat batch) and O3 (initial-leader spread) already make many small
groups affordable, which is what a shard-per-tenant fleet is.

## The migration: a hard, versioned cutover

Splitting the system log is a stored/replicated shape change, so it follows the
compatibility rules exactly (`docs/src/content/docs/operate/compatibility.mdx`,
`docs/src/content/docs/protocol/upgrades.mdx`):

- **Feature gate.** The new log shapes (a tenant-shard entry, a root/tenant
  routing record) are each a `Feature` in `system::cluster_version::FEATURES`
  with the release they ship in, refused `409 FEATURE_NOT_ACTIVE` below the
  cluster version. Old and new nodes share the single log through the rollout
  until the cluster version rises.
- **Declared cutover.** The point where the single group stops taking tenant
  writes and the shards take over is a one-way door: a `[[cutover]]` entry in
  `crates/node/api-deprecations.toml`, `!` in the release PR's title, and mdma's
  `CUTOVERS` line. Before the cutover the shards are shadow-built and verified;
  after it, the single group holds only root state.
- **Format versions.** A new entry shape raises `ENTRY_FORMAT`; the system
  group's snapshot gains the routing record, raising `SNAPSHOT_FORMAT`, with a
  reader for the old (single-group) shape and a test that reads it.
- **Upgrade-path case.** A cluster on the pinned release
  (`.github/upgrade-from`) with data written to the single group rolls onto a
  build that shards, and reads every tenant's state back — a new case in
  `crates/node/tests/fault/upgrade_cases/`, and a `DeepCase` for the
  user-visible multi-tenant behaviour.

The migration itself: a one-time split that reads the single system state and
writes each organization's slice into its new tenant group, leaving cluster-wide
state in the root group, coordinated by the root leader while the cluster version
holds the feature inactive for writers. The shards are built and digest-checked
against the source before the cutover makes them authoritative.

## The ladder (and where each step sits)

| Step | What | Scales with | Status |
| --- | --- | --- | --- |
| A | Leased authz: access served locally within a freshness bound (= O2) | removes read pressure entirely | **landed** (`Config::sys_read_lease`) |
| B | Shard by area | — | **not recommended**: the seam shears coupled commands (above) |
| C | Shard by tenant + root group | control-plane **write** rate | **this design** |
| D | Fold session writes into the tenant shard / leased revocation | login rate | folded into C |

Do A first (done). Do C when the control-plane **write** rate — mostly sessions
and app/org creation — outgrows one three-voter group, folding in D. Treat B as
an interim only; it does not scale with tenants.

## Scope classification (the first real obstacle)

Routing a command needs its [`Scope`]: which organization (if any) owns the
change. The obstacle, found while wiring this up, is that **scope is not a pure
function of the command**. Some commands name their organization directly
(`CreateOrg`, `UpdateTeam`, an org-scoped approval). Many do not: `CreateApp`
carries an `AppRecord` whose org is inside it, but `UpgradeApp`, `MoveApp`,
`ScopeApps` and the session/recovery commands name an **app id** or an **account
id** whose owning organization is only known by **reading system state**. So the
scope layer models four cases — `Cluster`, `Org(id)`, `OfApp(id)`,
`OfAccount(id)` — and resolving the last two to a tenant is a **read of the
state**, i.e. a cross-shard read once the owning org lives in a different shard
than the command is routed from. This is the same coupling that makes `CreateApp`
read the fleet quota and the release, and it is why the migration is staged
around the cross-shard-read design rather than a mechanical log split.

## What is in code today

`crates/node/src/system/shard.rs` is the routing layer, gated off:

- `ShardId` (`Root` | `Tenant(OrgId)`); `Scope` (`Cluster` | `Org` | `OfApp` |
  `OfAccount`); `Placement` and `area_placement(Area)` (the area table above).
- `tenant_of(scope, resolve_org)` resolves a scope to its owning organization,
  reading state through `resolve_org` for the `OfApp`/`OfAccount` cases (the
  cross-shard read; a `|_| None` resolver leaves it unresolved for now).
- `shard_for(active, tenant)` is the **gate**: it returns `Tenant(org)` only when
  sharding is active and the tenant resolved, otherwise `Root`. So wiring it into
  the routers is behaviour-preserving until a later slice turns the gate on.
- `resolve(&ShardId) -> GroupId` returns the single `SYSTEM_GROUP` for every
  shard, so **routing is unchanged**; a later slice gives each tenant shard its
  own group id here.

All of it is unit-tested and inert — nothing calls it on the hot path yet.

### Landed since (hard cutover, PR #5559)

The cutover is built without a feature gate: genesis starts empty, so there is
no migration and no old-format reader; the single-group path is removed, not
gated. In code and validated on the in-process `it` cluster (687 node tests
green):

- `SystemCmd::scope()` — the exhaustive classifier (mirrors `area`), wired
  through `sys_propose`; `shard::owns(role, tenant, active)`, the apply-side
  mirror of `shard_for`, with the invariant *a command is applied by exactly the
  shard its write routed to*.
- `SystemState::tenant_groups` (root-owned `OrgId -> GroupId`) and
  `next_tenant_group`: every organization is allocated a tenant group id from a
  dedicated high range (`shard::TENANT_GROUP_BASE`, `1 << 32`) on all three
  creation paths (`CreateOrg`, a sign-up's personal org, a recovery import), so a
  tenant id never shifts an app group id and is never reused.
- `Node::ensure_tenant_group` + the reconcile loop: every system voter now forms
  and hosts each org's tenant group (mirroring `ensure_app_group`), so tenant
  groups are **real, replicating Raft groups**. `admit_raft` admits a tenant
  group's Raft messages by the system group's membership (a tenant group is
  placed on the system voters; its id is in the dedicated high range), which the
  in-process cluster proved necessary — without it the group cannot reach a
  quorum.
- **The identity keystone** (`SystemCmd::SeedTenant`): once a tenant group forms,
  the node seeds it with its org record and a projection of its members'
  passkeys, read from the root directory. The tenant then verifies its members'
  consent from its own log alone (`Proofs::consent_of` reads the member
  passkeys), so `apply` stays deterministic and no node trusts a peer's read —
  the resolution of the gating decision above, validated end to end
  (`an_orgs_tenant_group_is_seeded_with_its_record_and_members`, 689/689 `it`).

What remains is the routing flip itself: role-aware `apply`, and routing an org's
reads and writes to its tenant group. The mechanism is proven; the work is
breadth. The org record is read in **~128 places across ~20 files and six
command areas** (orgs, apps, updates, approvals, recovery, deployments, plus the
access gate, `GET /orgs` and the consent page). Each read either moves to the
tenant machine (the apply handlers of org-detail commands, which now hold the
org) or becomes a resolve-at-propose that bakes the membership fact into the
entry (the cross-area reads). This is the pervasive, cluster-validated core — the
last step, deliberately not landed in one pass.

## Migration plan (staged)

Each slice compiles, is behaviour-preserving until the one that flips the gate,
and is reviewed on its own. Ordered so the hard design (cross-shard reads) is
settled before any state moves.

1. **Routing layer, gated off** *(this slice)*. `shard.rs`: scope model,
   `tenant_of`, the gated `shard_for`, `resolve`. No caller yet.
2. **Scope extraction.** An exhaustive `scope_of(&SystemCmd) -> Scope` mirroring
   `SystemCmd::area`, classifying every variant (the enum is large; this is where
   the per-command org lookup for `OfApp`/`OfAccount` is pinned down). Tested
   per-variant against the state the lookup reads.
3. **Wire the seam into the routers, still gated off.** `system/routing.rs` and
   `Node`'s system read/write paths ask `resolve(shard_for(active=false, …))`,
   which is `SYSTEM_GROUP` for every command — a no-op that removes the
   dead-code marker and proves the integration points.
4. **Cross-shard reads.** The mechanism for a tenant shard to read root state
   (fleet quota, a release) and owning-org lookups at a read index within O2's
   freshness model. This is the keystone; everything above is cheap, this is not.
5. **Tenant-group lifecycle.** Creating, opening, placing (reusing
   `placement.rs`/`grow_voters`), moving and retiring a per-tenant Raft group of
   the `SystemMachine`; routing a tenant's commands and reads to it.
6. **The feature gate.** A real `Feature` in `system::cluster_version::FEATURES`
   with its ship release, refused below the cluster version — deliberately **not**
   added until this slice, because an entry in that release-versioned array is a
   compatibility commitment that belongs with the code that first proposes a
   tenant-sharding command, not with scaffolding.
7. **The state-split migration + cutover.** Read the single system state, write
   each org's slice into its tenant group, digest-verify each shard against the
   source, then the declared one-way cutover (`[[cutover]]`, `!` in the PR title,
   mdma's `CUTOVERS`) with its `upgrade_cases/` + `DeepCase`.

Slices 1–3 are behaviour-preserving plumbing; 4 is the research; 5–7 are the
migration proper and ship behind the gate and the cutover. This PR is slice 1.

Having the routing decision in one typed, tested place lets the rest of the node
be written and reviewed against shards before any state is actually split.

## S2: the `SystemState` partition

`SystemState` is one flat struct of ~56 fields. S2 assigns each to the **root**
machine (cluster-wide, one instance) or a **tenant** machine (one instance per
organization, holding only that org's rows). A tenant instance applies only the
commands whose state it owns; a command that reads another shard's state has
those values resolved at propose and carried in the entry (S4), so `apply` still
reads only its own shard.

The rule: **state keyed by, or scoped to, one organization is tenant; everything
else — the fleet, the package registry, compute accounting, cluster governance,
and the global identity/directory indexes every shard must look up — is root.**

### Field classification

| Group | Fields | Shard |
| --- | --- | --- |
| Fleet | `nodes`, `removed_nodes`, `node_epoch`, `next_group` | **root** |
| Packages | `packages`, `bundles`, `publishers`, `unbound_publishers`, `package_origins`, `price_table` | **root** |
| Compute | `pools`, `compute`, `next_run_seq`, `run_served`, `run_grants`, `released_runs` | **root** |
| Governance (cluster) | `governance`, `operator_key`, `operator_releases`, `operator_outbound`, `release_deadlines`, `deployers`, `cluster_owner`, `org_quotas`, `quotas` | **root** |
| Cluster markers | `cluster_version`, `current_rules_from`, `session_proofs_from`, `attributes_by_release`, `expires_proofs` | **root** |
| Proof lifecycle | `used_proofs`, `proof_expiry`, `proofs_forgotten_through` | **root** (cluster-wide replay dedup) |
| Sign-up | `sign_up_invites` | **root** |
| Identity directory | `accounts`, `credentials`, `recovery_keys`, `activity`, `account_recoveries`, `backup_keys` | **root** (an account is a cluster-wide identity) |
| Org structure | `orgs` (members, teams), `invites` | **tenant** |
| Apps | `apps`, `app_invites`, `delegations`, `app_file_bytes`, `deployments`, `deployment_secrets` | **tenant** |
| Session family | `refresh`, `revoked_sessions`, `revoked_at`, `rotated_refresh`, `api_keys` | **tenant, by org context** (see below) |
| Byte charges | `payer_file_bytes` (keyed by org = `usage::payer`) | **tenant** |

### The crux: identity is root, sessions are tenant

The session family (`refresh`, `rotated_refresh`, `revoked_sessions`,
`revoked_at`, `api_keys`) is the **largest write source**, so it is exactly what
sharding must move off root — but each entry is tied to an **account**, which is
a cluster-wide identity (`accounts`, `credentials`, `recovery_keys` stay root as
a directory, since an account can act in several orgs).

Resolution: **a session shards by the organization context it is minted for.** A
sign-in scoped to an org (or to an app, which belongs to an org) mints its
session in that org's tenant shard; an account-level session with no org context
(cluster-wide operations) lives on root. This moves the hot session writes to the
tenants while keeping account identity a single root directory. The cost is the
access-gate read (S6): authorizing a request reads the **account identity from
root** and the **session/refresh record from the acting org's tenant** — a
cross-shard read at a read index, within O2's freshness model. A session cannot
span two orgs without re-minting; account-level actions fall back to root.

### What the partition forces

- **`apply` / `authorize` become role-aware.** `SystemCmd::area` already groups
  commands; each command applies on the shard that owns its state. A root command
  on a tenant instance (or vice-versa) is a routing error, not a no-op.
- **Resolve-at-propose (S4) for every coupled command.** `CreateApp` (tenant:
  `apps`) reads `quotas`/`org_quotas` (root) and the `orgs` membership (its own
  tenant) and the release (root `packages`); the leader reads root at a read
  index and bakes the quota and release facts into the entry.
- **The `IN_MEMORY` checkpoint splits.** The system machine checkpoints its whole
  state in each snapshot; after S2 each shard checkpoints only its slice, so
  `Machine::snapshot`/`restore` and the snapshot format carry a shard tag.
- **`next_group` stays root** (one id allocator), so tenant group creation (S3)
  asks root for the new group id.
- **`used_proofs` stays root** so a proof cannot be replayed on a second shard;
  a tenant decision's proof check is a root read at propose.

### Decisions (resolved)

- **Session org-scoping — yes.** A session is minted bound to the org it acts in
  and shards to that org's tenant group; account-level and cross-org operations
  fall back to root. The sign-in API's contract gains the org a token names, and
  a principal acting in several orgs holds a session per org. This is what moves
  the hot write load off root.
- **`activity`, `account_recoveries`, `backup_keys` — root.** They follow the
  cluster-wide account identity, so the recovery and activity paths stay
  single-shard (no extra cross-shard read).
- **`payer_file_bytes` — tenant, no discriminator.** The payer is the
  organization (`usage::payer(app)` = `app.org`): the map is a per-org rollup of
  file bytes, so it is tenant-owned keyed by org. Charging it reads the cluster
  `quotas` and the operator's `org_quotas` from root at propose (the `CreateApp`
  pattern).

## Implementation plan (S2–S8)

Concrete build order with the files and functions each slice touches. The hard
constraint, proven while building S1: **S2 + S3 + S5 must land as one unit** —
role-aware `apply` either breaks every org/app command (if it refuses them before
tenant groups and routing exist) or does nothing (if it doesn't). So the "core"
below is one coordinated change, validated on a live cluster; S4 interleaves with
it, and S6–S8 follow.

### S2 — role-aware `SystemMachine` (`system.rs`)

- Add `role: ShardId` to `SystemMachine` (`Root` | `Tenant(OrgId)`);
  `SystemMachine::new(cluster, role)`. Keep one `SystemState` type (code reuse),
  but each instance only writes the fields its role owns (the partition table
  above).
- `SystemMachine::run`/`apply` dispatch by `SystemCmd::area` + `role`: a root
  instance applies root-owned commands, a tenant instance its org's. A command on
  the wrong role is a routing bug (S5 guarantees it won't arrive), logged and
  failed, never applied as a no-op.
- `authorize` reads only this shard's state plus the facts baked into the entry
  (S4). No live cross-shard read inside `apply` — the determinism rule holds.

### S3 — tenant-group lifecycle (`node.rs`, `system/orgs.rs`, `placement.rs`)

- Root state gains `tenant_groups: BTreeMap<OrgId, u64>` (root-owned). `CreateOrg`
  apply (root) allocates a group id from `next_group` and records the mapping;
  `DeleteOrg` removes it.
- The node watches the root applied log; when the root state names this node a
  voter of a tenant group it does not host, it calls
  `engine.start_group(gid, SystemMachine::new(cluster, Tenant(org)))` and places
  voters across zones (`placement.rs`, `membership::grow_voters`, reusing the
  app-group machinery). At startup it opens every tenant group it should host.
- `Node::system_shard` already resolves `Tenant(org)` → the hosted group;
  populate `Node::tenants` here.

### S4 — resolve-at-propose (`system.rs`, `api/apps.rs`, `node.rs`)

- For each coupled command, the entry node reads its cross-shard dependencies at
  a system read index and bakes the values into the `SystemCmd` variant before
  proposing. `CreateApp`: read root `quotas`/`org_quotas` (the quota) and root
  `packages` (the release facts); the tenant `orgs` membership is this shard's own
  read. Add `quota_snapshot` / `release_facts` fields to `CreateApp` (and the
  equivalent to `UpgradeApp`, `InstallPackage`, …); `apply` checks the logged
  values, so every replica reaches the same answer.
- This is the standard "a leader may measure, then logs what it saw" pattern
  already used across `system/*`.

### S5 — route each command/read to its shard (`node.rs`, `api/*`)

- Implement `scope_of(&SystemCmd) -> Scope`, exhaustive over the enum (mirrors
  `SystemCmd::area`). For `OfApp`/`OfAccount` the owning org is read from the root
  directory at propose.
- `Node::sys_write`/`sys_propose` compute `scope_of(cmd)` →
  `shard::shard_for(true, tenant_of(scope, root_lookup))` → `system_shard` → write
  to that group. Reads that target tenant state (`sys` callers) take a scope; root
  reads stay on root.
- Genesis starts empty, so there is no bulk migration: new orgs create their
  tenant group (S3) and their commands route there from the first.

### S6 — access gate across shards (`node.rs`, `api.rs`)

- `Node::sys_fresh`/`sys_round` become per-shard (one read-index round per shard a
  request touches, under O2's lease model). The gate reads the account identity
  from root and the session/membership from the acting org's tenant, each fresh.
- `api::gate`/`principal` carry the request's org context so the membership read
  targets the right tenant.

### S7 — per-shard snapshots, recovery, backup (`log/machine.rs`, `backup.rs`, `recovery_disk.rs`)

- Each system shard is an `IN_MEMORY` machine checkpointed in its own snapshot;
  raise `SNAPSHOT_FORMAT` to tag a snapshot with its shard. Backup, recovery and
  `restore` iterate every system shard (root + tenants), not only group 1.
- Replace the remaining `SYSTEM_GROUP`-single assumptions (`member_addr`,
  `status`, `subscribe_applied`, metrics) with per-shard or root-only forms.

### S8 — tests, flip on, delete the old path (`tests/`)

- Multi-tenant cluster tests (`tests/it/`): two orgs' state lands in different
  groups led by different nodes; a cross-org operation is root-coordinated;
  failover of a tenant group keeps its org's control plane; a cross-shard access
  read is fresh under the lease. Process-suite tests for placement and failover.
- Fix the existing suite (much of it assumes one system group), then remove the
  single-group fallback: sharding is the only path.

### What validates where

The in-process cluster harness (`tests/it/cluster.rs`, real Raft + sealed
channels) proves functional correctness of each slice; **failover, placement
across zones, and split-brain safety of tenant groups need real `merod`
processes** (`tests/fault/`) and a multi-node deployment. That cluster validation
is why S2–S5 land as one reviewed unit on infrastructure this design note's
author did not have — it is the gate between this plan and production.

## Bring-up findings (cutover on, measured on the in-process cluster)

Turning the gate on (`shard::ACTIVE = true`) and running `component_cluster`
surfaces one dominant, well-understood failure, diagnosed command-by-command on
a live three-node cluster:

- **Org-scoped commands route correctly, invite commands do not.** Every
  `SystemCmd` whose `scope()` names its org explicitly (`CreateTeam`,
  `SetOrgRole`, `RemoveOrgMember`, …) routes to and applies on the org's tenant
  group — verified. But `CreateInvite` and `RedeemInvite` carry `Scope::Cluster`
  (they have no `org` field; the org is only reachable through the invite token),
  so they route to **root**. A redeemed member is therefore written into the
  **root** copy of the org and never the tenant copy. Reads go the other way: the
  access gate (`api::reaches` → `Node::org_of`) reads the org from its **tenant**
  shard, so the new member is absent from the org the gate judges against and
  every call/view/event for them is refused `404 NOT_FOUND` ("no such app" —
  `may_reach` with the member missing). This single gap accounts for the bulk of
  the `component_cluster` failures under the cutover (each builds membership via
  an invite, then exercises it).
- **The fix is C4 for the invite/membership family.** Route `CreateInvite` and
  `RedeemInvite` to the org's tenant so the invite record, its `uses_left`
  bookkeeping and the membership change all live on the one shard the gate reads.
  The one obstacle is that an invite **token names no org**: `RedeemInvite`
  cannot compute its tenant from the command alone. Resolve it at propose through
  a small **root-held `invite_org` directory** (`invite_key → OrgId`): the
  create-invite API writes the directory entry on root and the invite record on
  the tenant; the redeem API reads the directory to find the org, then routes
  `RedeemInvite` to that tenant, where it validates and decrements the tenant's
  own invite record and adds the member. The directory is routing metadata only
  (never decremented), so it introduces no second source of truth for
  membership. The same shape covers the other token-keyed, org-agnostic commands
  (app invites, and — once `Accounts`/`Recovery` flip — sessions and recovery).
- **But routing redeem to the tenant reaches the credential gate.**
  `RedeemInvite` applied on the tenant must (a) verify the redeemer's **consent
  proof** and (b) register the redeemer — both of which need that account's
  **identity** (its passkeys), which lives in the **root** account directory. The
  redeemer is, by definition, not yet a member, so the tenant seed has not
  projected them. This is the same blocker as the first open question below
  (verifying a credential inside a tenant), reached from the membership side. So
  the control-plane **membership path forks**, and the choice is a design/security
  decision, not an implementation detail:
  - **(X) Membership authoritative on root, tenant mirrors it.** Keep every org
    structure write (roles, teams, invites, redeem) on root, where account
    identity already is, and extend the one-shot tenant seed into a continuous
    root→tenant projection so the gate's tenant reads see current membership.
    Simple, no new credential mechanism, invariant 6 (identity from proofs on
    root) untouched — but org-structure writes are **not** sharded; the write
    relief comes from apps, sessions, compute and recovery, not membership. It
    also means reverting the org-structure commands that currently route to the
    tenant back to root.
  - **(Y) Membership authoritative on the tenant.** Route the whole org family
    (including invites/redeem) to the tenant for full write-sharding, which
    requires the **signed credential binding** below so a tenant can verify, from
    the entry alone, that a baked account/credential is authentic — without it, a
    faulty or compromised leader could forge consent. This is the higher-ceiling
    option and the one the rest of this note assumes, but it is gated on that
    security primitive.

  The slice landed so far (org read-routing, tenant lifecycle, role-aware apply,
  `CreateApp` org-bake) is compatible with either fork; the fork only decides
  where the invite/membership **writes** go.

### Progress with the cutover on (fork Y, attestation-level binding chosen)

The cutover was taken down fork **Y** with the binding trusted at the
**attestation** boundary the rest of the system already uses (per-node receipt
keys, the leader's logged measurements, the shipped tenant seed); consent is
always re-verified from the baked passkey. Landed and verified (each slice green
with the cutover off, so all behaviour-preserving until the final flip):

- **Invites and redemption on the tenant.** `CreateInvite` stores the invite on
  the org's tenant group; a root `invite_org` directory (`BindInvite`) maps the
  bearer token to its org so `RedeemInvite` routes there, carrying the redeemer's
  account identity so the tenant registers them from the entry. The peer-forward
  handler routes a forwarded command to its shard; the component host functions
  (`org-role`, `in-team`, `orgs-of`) read the org from its shard.
- **Access gates read the org from its shard.** `Node::sys_with_org` hands a gate
  the root state and the org from its tenant together; the release-approval,
  upgrade-policy, recovery, backup-key, egress, usage, team and app-transfer
  gates route through it, plus the invite list/revoke readers that moved with the
  invite records.
- **App-management commands bake the org.** `TransferApp`, `SetAccess`,
  `SetAppMetadata` (and `OfferApp`) carry the app's organization — or, for an
  offer, the release's serving organizations — read from their shards at propose,
  so `apply` on the root group re-judges membership against them.
- **Release visibility fans out across shards.** `member_of_with` /
  `sees_release_with` / `package_used_with` take an `extra` map of organizations
  the node resolved from their tenant groups; the install, registry-install,
  offer and upgrade gates build it with `Node::release_org_view`. A member can
  see and instantiate a private release their organization serves.

This cleared the large majority of the `component_cluster` failures under the
cutover. **Remaining, and genuinely cross-tenant** (a single read spanning many
organizations, which no one shard holds — the design's hardest shape):

- **Deployment serving across customers.** Deciding which instances a changed
  offer moves, and what update is available, reads the membership of every
  organization the deployment serves at once. It needs a fan-out over the
  deployment's customer organizations (each on its own tenant), in the
  available-update gate and the move path — the same `release_org_view` shape
  widened from one release's orgs to a deployment's customers.
- **The legacy `sees_release` branch** (releases recorded before `visibility`)
  still scans all organizations on root; a genesis-sharded cluster records
  visibility on every release, so it is unreached there, but it is not sharded.
- **The deployment customer report** (`api/deployments.rs`) aggregates member
  counts across every customer organization — the same fan-out.

## Open questions / risks

- **The gating decision: verifying a credential inside a tenant.** This blocks
  the routing flip. A tenant command's authority is a consent proof
  (`proof::verify_consent`) checked against the actor's **credentials**, which
  live in the **root** account directory (one account acts across many orgs). A
  tenant group hosting the command's `apply` does not hold that directory. Baking
  the credential into the entry at propose is not enough on its own: `apply` must
  judge authority on every node from the log alone (invariant: *a node that only
  sees the log cannot forge a decision*), so a faulty or compromised leader that
  baked a forged credential-plus-consent pair would be believed. The flip needs a
  way for a tenant to verify, from the entry alone, that a credential is bound to
  its account — e.g. a **signed credential binding** (operator- or cluster-key
  signed when the credential is registered on root, verifiable by any shard)
  carried in the entry. This is a genuine security-design choice for the
  attestation/identity model and must be decided before any tenant applies a
  consent-bearing command; it is deliberately not improvised. Account-level
  sessions and `SetOrgMetadata` (no consent proof) do not hit it, but the hot
  org/app/session commands do.
- **Root read freshness vs. a tenant write's latency.** Reading the fleet quota
  from the root group on every `CreateApp` reintroduces a cross-group round;
  caching it needs a staleness bound the quota semantics accept. Quantify before
  committing to synchronous root reads.
- **Account identity placement.** Whether the account-existence index lives in
  the root group or is replicated per tenant changes the sign-in path; both
  satisfy invariant 3 but with different latency.
- **Rebalancing.** Organizations are created and deleted continuously; a
  tenant-shard-per-org fleet needs the placement and move machinery to retire and
  relocate shards, not only create them.
- **Blast radius of the cutover.** The split touches the most central log in the
  system. The shadow-build-and-verify step, the digest check against the source,
  and the upgrade-path case are the safety net, and must come before any node
  treats a shard as authoritative.

This is the highest-risk item in the performance design and the last
non-sharding ceiling; it is deliberately design-led, and this note plus the seam
are its starting point, not a landable migration.
