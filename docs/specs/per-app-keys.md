# Per-app storage keys: design proposal

Oct 29, 2026 · Status: **proposal, not implemented** · For review by Xabi (runtime and storage) and Sandi

This proposes how an app's data on a cluster comes to be sealed under keys of its
own instead of under the cluster's one storage key K_r. It is the item "keys per
app from mero-kms" in `crates/node/AGENTS.md` ("Not done yet"), and it touches the
other one beside it, the rollback counter. No code changes with this document.

Short version: per-app keys are worth doing on the shared cluster, mainly for
deleting an app for good (crypto-shredding), for keeping a leaked key or backup
to one app, and for TEE keys that survive a mero-tee release. They do **not**
keep a fully compromised node out of other apps, because a node is the trusted
computing base: it holds the session-signing key and is a trusted peer. Asking
mero-kms for a key per app (option B in the brief) also runs into three facts
about mero-kms as it is built today: it is frozen, it keeps no state, and its
root changes with every release. The recommendation (section 9) is a random data
key per app, which the app's replicas hold and hand to each other over the
attested peer channel, and which each node wraps under a key of its own from
mero-kms. The first phase makes no change to mero-kms.

## 1. Today, from the code

| What | Key | Where |
| --- | --- | --- |
| K_r | `HKDF-SHA256(ikm = KMS key, salt "calimero.cloud.storage.v1", info = cluster)` | `crates/node/src/kms.rs`, `derive_storage_key` |
| KMS key | `HKDF(root, "{prefix}/{profile}/{peerId}")`, where `peerId` is the disk identity that every node of the cluster shares | mero-kms `handlers/get_key.rs`, `key_path_for_peer`; `backend.rs` |
| Every value in RocksDB: Raft entries (`log`), metadata (`meta`), state rows (`state`), blobs and bundles (`blobs`) | AES-256-GCM under K_r itself, with AAD = column tag, group and key | `crates/log/src/store.rs`, `Db::sealer` |
| Hidden row keys (app state `rows::KV`/`CKV`, idempotency `IDEM`/`CIDEM`) | `HMAC(K_r, "calimero.log.hidden-keys.v1")` | `store.rs`, `Db::hider` |
| Snapshot frames, and the backups that copy them | `HMAC(K_r, "calimero.log.snapshot.v1")`; rows inside stay sealed under K_r | `store.rs`, `SnapshotKeys`; `crates/node/src/backup.rs`, `latest` |
| App TEE key (`TeeSecret`, `#[app::tee]` state) | `HMAC(K_r, "calimero.tee.v1\|" ‖ app)` | `backup::tee_key` |
| Member stand-in key | `HMAC(K_r, "calimero.member.v1\|" ‖ app ‖ "\|" ‖ account)` | `backup::member_key` |
| Cluster secret (`x-cluster-auth` on `internal/*`) | `HMAC(K_r, "calimero.cluster.v1\|" ‖ cluster)` | `node.rs`, `Node::start` |
| Compiled-component cache MAC | from K_r | `node/component.rs`, `component_cache_key` |

So one 32-byte key, which every node of the cluster holds in memory, opens every
app's log entries (each holds the run's diff in the clear inside the sealed
value), state, events, idempotency records, blobs, snapshots and backups, and
derives every app's TEE key. `docs/.../protocol/encryption.mdx` says this ("today
one K_r covers every app on the cluster"), and so do `security-model.mdx` and
`key-rotation.mdx` ("K_r ... not rotated").

Facts from mero-kms (`calimero-network/mero-tee`, `mero-kms/`, and
`docs/design/gcp-tdx-kms.md` on master) that constrain any design:

- **Frozen.** The node allowlist is baked into the KMS image. No party can change
  which code gets keys once the image is built. A new rule is a new KMS release.
- **Stateless.** The only secret is a random root in the replicas' memory. Replicas
  share no storage, and challenges are stateless (`stateless_challenge.rs`). The
  KMS keeps no record per key, so it cannot keep a counter, a revocation list or
  a placement table without a new replicated store.
- **One root per release.** "Upgrade to N+1: a new node image, a new KMS image, a
  new cluster, a new root." Every key the KMS derives changes at every release,
  and retiring a release's KMS makes its keys unrecoverable.
- **Keys by identity, not by node.** `/get-key` releases the key for whichever
  libp2p identity signed the request, to any TD that matches the allowlist. The
  cluster's nodes share one disk identity, so the KMS cannot tell them apart.
- The design doc already warns: "TEE-only secrets must not depend on a release's
  root ... The vault key has to pass from old-release TEE nodes to new-release ones
  by attestation, at the protocol level."

The last two facts matter today, before any per-app work. The app TEE key derives
from K_r, and K_r derives from the release root, so a value sealed to an app's TEE
cannot be opened on a node of the next release. (Section 12, Q1.)

## 2. Threat model delta

The shared cluster puts many developers' apps on standard TDX nodes that Calimero
runs, with Calimero as the operator and the host. The question is what per-app
keys change compared with one K_r, attacker by attacker.

| Attacker | With one K_r | With per-app keys (recommended design) |
| --- | --- | --- |
| **Host / operator reading disks and sealed backups** (Calimero on the shared cluster) | Reads nothing without K_r. A K_r leaked later opens every disk and backup ever taken. | Reads nothing either. A key leaked later opens only that app's data, and **a deleted app's data never opens again** once its key is shredded (section 6). |
| **Passive disclosure of key material** on one node: a memory-read bug, a key in a log or a core file, a side channel that recovers bytes | Leaks K_r, and with it every app on every node's disk, every backup and the cluster secret. | Leaks the node's own wrapping key and the data keys of the apps that node hosts. Other apps stay sealed, including on that node's own disk once their keys have been dropped (section 5). |
| **App escaping the WASM sandbox into `merod`** (a malicious tenant on the shared cluster) | Full control of the node: K_r, K_s (the session-signing key, in the system state), the cluster secret, every app. | **No real change.** The node still holds K_s and is still a trusted peer: it can mint a session for any account, or forward a call as anyone, and read any app through the API. It also holds every hosted app's plaintext in memory. Per-app keys only remove the offline path, decrypting other nodes' disks or backups. |
| **Remote code execution in `merod`** through the API or the peer channel | Same as above. | Same as above. |
| **Cross-tenant read through a node logic bug** (a run, a view or a scan reads another group's rows) | Already partly stopped: the AAD binds group and key, but any code path with the `Db` opens any group. | A second wall: a group's store handle holds only that group's key, so a read through the wrong handle fails authentication instead of returning rows. Defence in depth, cheap. |
| **Operator moving an app to a node it controls** | Stopped by release approvals (`Node::may_send`). | Unchanged. The key travels under the same gate as the data. |
| **A compromised mero-kms or release root** | Every key of that release. | Every key of that release too, unless the owner holds a wrapping key (option C). |
| **Host restoring an old disk (rollback)** | Not detected. | Not detected by keys alone. Section 7 ties in a counter. |
| **A removed member** | Holds no key; access judged per request. | Unchanged. |

So the honest claim is narrower than "a breach of one app cannot reach the
others". **Active** control of a node still reaches every app on the cluster,
because a node is the trusted computing base (`security-model.mdx`, "A node of the
cluster: —"). Against that, the lever is placement, not keys: separate node pools
per tenant class on the shared cluster, or dedicated clusters. Section 12, Q9,
asks whether placement isolation should come first. Per-app keys buy:

1. **Deletion that holds**: crypto-shredding on `DELETE /apps/{app}` and on org
   deletion. This is the main product reason on a shared, multi-developer cluster.
2. **Leaks confined to one app**: a disclosed key, a leaked backup or an exported
   snapshot reaches one app, not the cluster.
3. **Rotation per app**: re-key one app (after an incident, a transfer, or on a
   schedule) without touching the others or every node.
4. **TEE keys that survive a release**: the fix for the release-root problem in
   section 1.
5. **A place for owner-held keys later** (BYOK on dedicated clusters, the planned
   organization recovery seed for backups).

## 3. Key hierarchy options

### A. Derived locally: `K_app = HMAC(K_r, "calimero.app.v1|" ‖ app ‖ "|" ‖ epoch)`

- Isolation: none against anything that yields K_r, which re-derives every K_app.
  It only gives the per-group store handle (the logic-bug wall).
- Rotation: cheap, by bumping `epoch`.
- **Crypto-shredding: none.** A deleted app's key is re-derivable by anyone who
  holds K_r. Adding a random per-app salt kept in the system state helps only
  once every copy of that salt is gone, and the salt sits in every node's
  system-group snapshot and in each node's memory.
- Release rollover: inherits K_r's problem, since K_app changes when the root does.
- Cost: no mero-kms change, no network, nothing on the wire.

### B. Released by mero-kms: `K_app = HKDF(root, "merod/app/{profile}/{cluster identity}/{app}")`

- Isolation: only as good as the KMS's check of **who may ask for which app**.
  The KMS is frozen and stateless, and it cannot read the system group's
  directory. It would release any app's key to any TD that holds the cluster
  identity, and a compromised node holds it. For a real gain the KMS would have
  to verify a **placement certificate**: a statement that node X hosts app Y,
  signed by a quorum of system-group voters whose signing keys the KMS
  attests itself. On top of that, every node would need an identity of its own.
  This is a lot of new code in a component whose bugs can be fixed only by a new
  release.
- **Crypto-shredding: none.** The KMS re-derives the key for whoever asks, for
  as long as the release's KMS runs. Data is shredded only when the whole
  release retires.
- Release rollover: the key changes every release, so every app's data must be
  re-sealed on every rollover, and an app's TEE values are lost.
- Cost: one quote and a KMS round trip per app at open. A node hosting thousands
  of apps on the shared cluster would need thousands at boot, or a batch
  endpoint. If the KMS is down, apps cannot open.

### C. Owner-held wrapping key (BYOK), for dedicated clusters

- The app's data key is additionally wrapped under a key the owner controls: a
  key derived from a passkey through WebAuthn's PRF extension, an organization
  recovery seed (already planned for backups in `operate/security.mdx`), or a
  key in the customer's own KMS.
- The cluster needs the unwrapped data key in memory to run timers, TEE handlers
  and xcalls without the owner, so the owner releases it once into the TDs, and
  again after the replicas lose it. **Revocation** means not providing it again:
  the data stays sealed from the next time every replica restarts, or at once if
  the owner also deletes the app.
- It protects against a compromised release root (B's and D's common trust) and
  gives the owner exportable backups.
- Cost: an owner ceremony at creation and whenever every replica restarts, and an
  availability dependency on the owner. It does not fit the shared cluster's
  defaults. It is an option for dedicated clusters, layered on D.

### D (recommended). A random data key per app, held by its replicas, wrapped per node

- **Data key.** `DEK_app,v`: 32 random bytes, generated by the app's first
  leader when the group is created, with version `v` starting at 1.
- **Who holds it.** The app's replicas, and nobody else. A new replica receives
  it from the app's leader over the peer sealed channel, under the same
  `may_send` release-approval gate as Raft entries and snapshots, as part of the
  byte hand-over that #4741 adds to replica moves (`Node::hand_over_bytes`).
- **At rest.** Each node keeps a local keyring in its `meta` column: `group →
  [(v, wrapped DEK)]`. Entries are wrapped under **K_n, a key of that node
  alone** from mero-kms. That needs a node identity of its own (`--node-kms-identity`,
  created per node, never copied), which the existing `/get-key` already
  supports, since the KMS derives a key per identity. K_r stays for what is per
  cluster (section 4).
- **Isolation.** A node's disk opens only with its own K_n, and holds only the
  keys of apps it hosts or hosted until they were dropped. Leaking K_r no longer
  opens app data. Leaking K_n opens one node's hosted apps.
- **Crypto-shredding.** Delete the keyring entries on every replica (section 6).
  Nothing can re-derive a DEK.
- **Release rollover.** The DEK passes from release N's replicas to release N+1's
  over the attested channel, as the data already does. TEE values survive.
- **Cost.** Each node calls mero-kms twice at boot (K_r and K_n), or once with a
  batch endpoint (section 8), and makes no per-app KMS call. Opening an app means
  unwrapping 32 bytes locally.

D is what B was meant to achieve: an app's key exists only where the app is
placed, without the KMS having to know placement. A frozen KMS cannot know
placement. The replicas already do, through the system log.

## 4. What is keyed per app, what stays per cluster

Per app (under `DEK_app,v`, or keys derived from it with HMAC labels):

| Data | Today | Proposed |
| --- | --- | --- |
| App group's Raft entries (`log` column; each carries the diff) | K_r | `seal(DEK)`; AAD unchanged (`log ‖ group ‖ index`) |
| App group's metadata (`meta`: vote, applied, membership, snapshot pointer) | K_r | **Stays under K_n** (it holds no app data, and the node needs it before the keyring) |
| App state rows (`rows::KV`, `rows::CKV`, private spaces, writer sets, blob marks under `\0calimero.*`) | K_r; row key hidden with HMAC(K_r) | DEK; hidden-row HMAC key `HMAC(DEK, "calimero.log.hidden-keys.v2")` |
| Idempotency (`rows::IDEM`, `CIDEM`, `corder`, `expiry`) | K_r | DEK (same rows as state) |
| Event ring (`rows::EVENT`) | K_r | DEK |
| `Head` and `ComponentHead` | K_r | DEK |
| Snapshot frames and the rows inside, so also backups | `HMAC(K_r, "calimero.log.snapshot.v1")` and K_r | `HMAC(DEK, "calimero.log.snapshot.v2")`; the frame header names the DEK version |
| App blobs (`env::blob_*`, `/apps/{app}/blobs`) | K_r, one copy per content hash, shared across apps | DEK; stored per app: blob name `app/{hash}`. This gives up cross-app dedup, and the "known window" in `collect_blob` (another app's upload losing its bytes) goes away with it. |
| App TEE key and member stand-in keys | `HMAC(K_r, ...)` | `HMAC(DEK_app,1, "calimero.tee.v2")`, pinned to version 1, see Q2 |
| Search indexes | In memory, derived, nothing at rest | Unchanged. If they come to survive restarts (a "not done yet" item), under DEK |
| Presence | In memory only | Unchanged |

Per cluster (K_r), unchanged:

- The **system group**: entries, its in-memory machine's checkpoints in snapshots.
  It holds accounts, credentials' hashes, K_s and the directory, and every node
  needs it.
- The **cluster secret** for `internal/*`.
- **Bundles and release `.mpk` files**: code shared by apps, and fetched from the
  registry anyway.
- The **compiled-component cache MAC**, though it could move to K_n, since it is
  per node.

Per node (K_n, new): the keyring, and the app groups' `meta` rows.

Two things stay as they are. Members still hold no key (members read through
the node, as today). Data in transit keeps the peer sealed channel, so no
per-app transport encryption is needed: Raft entries already travel inside the
channel, and each node re-seals them at rest.

## 5. Placement: getting and dropping keys

- **Create.** `CreateApp` names the replicas. The first leader generates `DEK_1`
  before `Init`, wraps it into its own keyring, and hands it to the other
  replicas before they need it. A follower that receives an entry for a group it
  has no key for asks the leader (`internal/apps/{app}/key`). It refuses to apply
  until it has the key, and never seals the entry under anything else.
- **Move onto a node** (`POST /ops/apps/{app}/replicas`, the rebalancer, #4741).
  Step 3, "add each new node as a learner", gains a step before it: the leader
  sends the keyring (every version still in use) with the bundle and blobs
  (`hand_over_bytes`), only to targets that passed the release-approval check.
  A learner without the key cannot install the snapshot.
- **Move off a node.** When reconcile stops the group and deletes its data, it
  deletes the keyring entry in the same batch. Until then the node still has the
  key. Q5 asks whether a node that leaves should be told to drop the key at once,
  so that a stopped node does not keep it on disk.
- **Late start** through the group factory: the key comes with the first
  snapshot, or is fetched from the leader, as above.
- **Rebalancer cost.** One small extra message per move. Keys are 32 bytes per
  version.
- **Restart.** K_n comes from mero-kms, the keyring is unwrapped, and the groups
  start. No network call per app.

## 6. Deletion is crypto-shredding

`DELETE /apps/{app}` (and `DELETE /orgs/{org}`, `DeleteTeam`) already make
reconcile remove the group's data from every replica. With per-app keys:

1. Each replica deletes its keyring entries for the group, synced, before it
   deletes the rows. Deleting the rows is then only space reclamation: RocksDB
   compaction may keep old SST files for a while, and they are useless without
   the key.
2. Backups taken before the deletion carry only the sealed snapshot. Their key is
   the question in Q4. If a backup carries the DEK wrapped under anything a
   cluster node can still open, shredding is only as final as the oldest backup.
   The proposal: a backup carries the DEK wrapped under a **backup key**, either
   a per-org recovery key or `HMAC(K_r, "calimero.backup.v1|" ‖ app)` until that
   exists, and the backup retention period bounds how long shredding takes. Docs
   should state the window ("deleted data is unreadable everywhere within N days").
3. A replica that is down at deletion time still holds the key until it comes
   back and reconciles. A node that never comes back keeps a sealed copy under
   its own K_n until its disk is destroyed, which mdma does when it retires a
   node.

What crypto-shredding does not cover: plaintext a member already read, and any
data an app sent out (outbound HTTPS, xcalls into other apps).

## 7. Rotation and epochs (and the rollback counter)

**Rotation of an app's key.** A system command `RotateAppKey { app }` (owner,
org admin with `MANAGE_APPLICATION`, or the operator) bumps
`AppRecord::key_version`. The app's leader generates `DEK_v+1` and sends it to
the replicas. New values are sealed under the newest version, and every sealed
value names its version (section 10). Old versions are dropped once nothing
references them, that is, after the next snapshot of the group plus a local
re-seal pass (section 9, migration) and once the log has been purged past the
last entry sealed under them. This also gives the dual-read window for
migration.

Rotation is not needed when a member is removed (members hold no key). It
**should** happen on `TransferApp` to another organization (Q6), and it is how an
incident is handled.

**Release rollover.** Release rollover does not re-key apps (D passes keys
across releases). K_r and K_n change with the release, and only the keyring is
re-wrapped, because the new node receives DEKs from peers.

**Rollback counter.** The note in `AGENTS.md` proposes "a mero-kms epoch, restored
nodes rejoin as learners". mero-kms is frozen and stateless, so an epoch kept
*in* mero-kms needs a replicated, persistent counter among KMS replicas, which
the KMS design deliberately avoids (no disk, root only in memory). That is a
large change to the one component that cannot be patched in place. An
alternative that per-app keys make cheap:

- The **system group** records, per node, an `incarnation` counter that the
  node bumps at every start (`NodeStarted { node, incarnation }`), and, per
  app, `key_version`.
- At start, before it votes or serves anything, a node compares the incarnation
  its disk holds with the one the system group returns, through `sys_fresh`
  over the attested channel. If its disk is behind, it was restored from an
  older copy. It then wipes its app groups' votes and logs and rejoins them as a
  **learner**, which is the rejoin the note describes, and it rejoins the system
  group as a learner too.
- A restored keyring holding only key versions the system state has retired is
  further evidence, and those versions no longer exist on the live replicas.
- This detects a rollback of a minority of nodes. A rollback of a majority, or
  of the whole cluster, is not detected, and nothing that lives only inside the
  cluster can detect it. That needs an outside witness, which is where a mero-kms
  epoch would fit later: K_n derived with an epoch the KMS serves and only ever
  raises.

Q7 asks whether to keep the "mero-kms epoch" wording or move to this.

## 8. mero-kms API changes

**Recommended path (D): none for phase 1.** A node fetches two keys through the
existing `/get-key`: the cluster key (shared disk identity, as today) and its
node key (its own identity). Optional improvements, in mero-tee:

- **`POST /get-keys`**: one challenge, one quote, one signature over
  `[identity₁, identity₂]`, returning both keys sealed. It halves boot-time
  quotes. The quote's report data commits to `SHA-256` over the canonical list.
  Limit `MAX_IDENTITIES_PER_REQUEST = 4`.
- **Rate limits**: per peer id per minute on `/get-key`. A node fetches at boot
  only, so a low limit (for example 30 per minute) costs nothing and slows any
  attempt to farm keys. The spent-challenge cache already bounds replay.

**If B were chosen anyway**, the endpoint would be `POST /get-app-keys
{ challengeId, quote, clusterPeerId, signature, apps: [appId…] (≤ MAX_APPS_PER_REQUEST),
placement: <certificate>, sealTo }`. The key path would live under a namespace
of its own, salted apart from node paths: `backend.rs` notes that node paths
come from config, so no other path is guaranteed never to collide. The policy
would bind the cluster identity's signature, the TD's measurements, and a
placement certificate the KMS verifies against system-voter keys it attested
itself. App ids would be logged next to peer ids, and the operator can read
those logs (`mero-kms/README.md`, "Logging"). App ids are random, but they are
per-customer identifiers. All of this would ship in a frozen image.

## 9. Recommendation and phased plan

Adopt **D**. Keep **A** only as the in-code shape of phase 1 if Xabi prefers
smaller steps (Q8). Defer **C** to dedicated clusters. Do not build **B**.

**Phase 1: per-group keys in `calimero-log`, keys from the node.** This is the
smallest useful step.

- The store gets a keyring: `Db` holds K_r (system group, cluster), K_n (meta,
  keyring), and per group a `GroupKeys { versions: BTreeMap<u32, Sealer>, hider,
  snapshot }`. `Engine::start_group` takes the group's keys, and the
  `StateTx`/`StateRead` handles of a group seal and open only with that group's
  keys. This is the logic-bug wall.
- The sealed-value format gains a header (section 10). Values without a header
  open under K_r (dual read).
- The node generates a random DEK per app at creation, wraps it under K_n, and
  hands it over on create and on moves (#4741's hand-over).
- Blobs go per app. TEE and member keys stay on the K_r derivation for existing
  apps and move to the DEK for new apps (Q2).
- Delete shreds. `merod inspect` takes K_n.
- No mero-kms change: two `/get-key` calls at boot. mdma creates a per-node
  identity next to the cluster one.

Phase 1 delivers crypto-shredding, leaks confined to one app at rest, and TEE
keys that survive a release for new apps.

**Phase 2: migration and rotation.**

- A local re-seal pass moves existing apps off K_r: write the next state
  generation under the DEK and switch in one batch, as snapshot install already
  does. Old log entries age out with the purge.
- `RotateAppKey`, and the system state's `key_version`.
- Backups carry the DEK wrapped under the backup key (Q4).

**Phase 3: rollback detection.** Node incarnations in the system group, restored
nodes rejoining as learners (section 7).

**Phase 4 (dedicated clusters): owner-held wrapping (C)**, joined with the
organization recovery seed for backups.

**Separately: placement isolation for the shared cluster** (tenant pools), the
only lever against an actively compromised node (Q9).

## 10. Formats, compatibility and the tests they need

On-disk changes:

1. **Sealed value header**: `0xC1 ‖ key_kind (u8: 0 = K_r, 1 = K_n, 2 = DEK) ‖
   version (u32 BE)`, then nonce and ciphertext. The header goes into the AAD,
   so a value cannot be relabelled to another key. A value without the header is
   legacy: it opens under K_r. A random nonce has a 1/256 chance of starting
   with `0xC1`, so the legacy reader cannot just test the first byte. It tries
   the header first and falls back to K_r on failure, or better, each column
   records the format it was written in, in `meta`. This is Q3 for Xabi.
2. **The keyring**: rows in `meta` under `group ‖ "keys"`, sealed under K_n.
3. **Snapshot frame header**: names the DEK version. An old-format snapshot opens
   under K_r.
4. **Blob names**: `app/{hash}` for app blobs, while bundles keep `{hash}`.
5. **The system state**: `AppRecord::key_version`, and in phase 3
   `Node::incarnation`, with serde defaults. Old states read as version 1, "keys
   not yet migrated".

On the wire:

- `internal/apps/{app}/key` (peer to peer, sealed channel, cluster secret). This
  is a new input from peers. It needs a named limit, and its answer goes only to
  a node the directory lists as a replica of the app, on an approved release.
- **Mixed versions during a rolling upgrade.** A snapshot sealed under a DEK
  cannot be opened by a node of the previous `merod`. The feature is therefore
  switched on cluster-wide by a system command only once every node runs a build
  that supports it, like a minimum-version gate (Q10).

Tests (CLAUDE.md: merobox runs one build on fresh state, so these need their own
tests):

- **Migration test**, as in `crates/log/src/migration_tests.rs` and
  `crates/node/tests/old_datadir.rs`: a data directory written by the current
  release (K_r only) opens under the new build, every app reads back, a write
  lands under the DEK, the re-seal pass leaves no value under K_r for app groups,
  and the old K_r vectors still open (`fixtures/known_answers.txt` gains vectors.
  Old ones are never changed).
- **Known-answer vectors** for every new derivation and for the header layout,
  as `the_storage_key_derivation_is_unchanged` does today.
- **Isolation**: a group's handle cannot open another group's row (expects an
  authentication error, not rows). No app data under K_r on disk after migration
  (extends "no plaintext on disk").
- **Shredding**: delete an app, then the keyring entry is gone on every replica,
  and its snapshot file or backup does not open with K_r or K_n.
- **Moves**: a move to a new node carries the key, and a target on an unapproved
  release gets neither data nor key. After a move off, the old node's keyring
  has no entry.
- **Rotation**: values written under v and v+1 both read until the re-seal,
  after which v is gone.
- **A release rollover in-process**: two K_r values, data moved, and a TEE value
  sealed before it opens after it.

## 11. Performance and failure modes

- **Crypto.** A ring `LessSafeKey` per group and version, built once when the
  group starts. AES-GCM costs the same per value as today. Memory is about 1 KiB
  per hosted app.
- **App open.** One local unwrap, no network. Under B it would be a KMS round
  trip with a quote per app, with TDX quote generation typically taking
  milliseconds or more.
- **Boot.** One more `/get-key` (or one `/get-keys`). This is the only new KMS
  dependency.
- **KMS down at boot**: as today, the node cannot open its disk and does not
  start. **KMS down while running**: nothing changes, because the keys are in
  memory and moves need no KMS. **KMS down at app open**: not a case under D
  (under B the app would be unavailable). **Every KMS replica lost**: as today,
  a node of that release that restarts cannot open its disk. It rejoins as a new
  node and receives the DEKs from peers, which works as long as one replica of
  each app is still running.
- **A replica without the key** (the hand-over was lost) refuses to apply and
  asks the leader. If no node holding the key is left, the app is lost, which is
  the same as losing every replica's data today. Backups (Q4) are the recovery
  path.
- **Blobs** lose cross-app dedup. On a shared cluster, the same bytes uploaded to
  two apps cost twice. In exchange, the content-hash oracle across apps goes away.

## 12. Open questions for Xabi

1. **TEE keys and the release root.** Today `backup::tee_key` derives from K_r,
   which changes with each mero-tee release (a new KMS root), so a `TeeSecret`
   sealed before a rollover does not open after it. Is that already handled
   somewhere I missed (the runbook's "Not confirmed here" caution suggests not)?
   If not, should the DEK-derived TEE key be phase 1's first deliverable, ahead of
   everything else?
2. **Existing apps' TEE and member keys.** App code holds values sealed to the
   current TEE key, and the host cannot re-seal them. Should existing apps keep
   `tee.v1` from K_r for good, with only new apps on `tee.v2` from `DEK_1`, or
   should the app carry both and let the SDK re-seal on read?
3. **Sealed-value header.** In-band magic plus a fallback, or a per-column
   format marker in `meta`? And should the store handle hold the group's keys
   (`Engine::start_group(keys)`), or should `Machine` supply them?
4. **Backups.** Under which key does a backup carry the DEK: a per-org recovery
   key (needs that work first), a K_r-derived backup key (shredding then bounded
   by retention), or nothing (backups as useless as the node once the key is gone)?
   What retention does mdma keep?
5. **Leaving a node.** Should a node that a move takes an app off drop the key at
   once (a `ForgetKey` peer message), or is "deleted when reconcile stops the
   group" enough?
6. **Rotation triggers.** Rotate on `TransferApp` between organizations? On a
   schedule? Who may call `RotateAppKey`: the owner only, or the operator too?
7. **Rollback counter.** Keep "a mero-kms epoch" (which needs persistent,
   replicated state in a frozen, stateless KMS), or move to node incarnations in
   the system group, which detect a minority rollback only?
8. **Order of work.** One step to D (random DEK and K_n), or first A (derived
   from K_r) to land the store plumbing, then D? A alone buys only the logic-bug
   wall and costs a second migration.
9. **Placement isolation.** Is tenant-pool placement on the shared cluster (the
   only defence against an actively compromised node) in scope before or
   alongside this? Per-app keys do not stop it.
10. **Mixed-version clusters.** A cluster-wide switch in the system state after
    every node upgrades, or tie it to `--peer-min-release`?
11. **Components and generations.** The component executor keeps state in memory
    and in `rows::CKV` per generation (D39, D41). Does a rebuild's new generation
    (D38) want to start under a new key version, so that the switch doubles as a
    rotation and the dropped generation is shredded with its key?
12. **Per-node identity.** Is mdma creating a second, per-node KMS identity
    acceptable, or should K_n come from a new KMS path under the cluster identity
    plus the node's attested transport key, which needs a mero-kms change?
