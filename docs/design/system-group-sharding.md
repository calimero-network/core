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
| Byte charges | `payer_file_bytes` | **split**: an org payer → tenant, an account payer → root |

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

### Open questions this raises (for review before S2 code)

- **Session org-scoping.** Pinning a session to one org at mint changes the
  sign-in API's contract (a token names its org). Account-level sessions on root
  reintroduce some write load there — quantify what fraction stays root.
- **`activity`, `account_recoveries`, `backup_keys`** follow the account to root
  here; if an org wants its own activity/recovery isolation they move to tenant,
  at the cost of another cross-shard read on the recovery path.
- **`payer_file_bytes` split** needs a key discriminator (org vs account payer)
  the current flat `String` key does not carry.

## Open questions / risks

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
