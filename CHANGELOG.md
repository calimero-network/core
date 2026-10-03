# Changelog

## [Unreleased]

### Changed

- **TEE admission is bound to the admitted credential and fresh.** A fleet TEE
  node is admitted only on a quote whose report data is a challenge the
  admitting member chose for it (32 random bytes, single-use, valid for about
  a minute), followed by `SHA-256("calimero.tee.admission.v1" || namespace ||
  group || identity key || account || delivery key || device)`. The node asks
  for the challenge over the direct admission request path, or a member offers
  it after hearing the node's prompt: `TeeAttestationAnnounce` and
  `TeeReleaseAttestationAnnounce` are replaced by a quote-free
  `TeeAdmissionPrompt`, which admits nobody. `RootOp::MemberJoinedViaTeeAttestation`
  carries its quote and every peer checks it against the credential in the op;
  `GroupOp::TeeAuthorityEvidence` carries the credential its quote was made
  for; the admitting node refuses a quote already used in an admission or an
  evidence refresh in the namespace.
  Breaking: wire and signed-op layouts change
  (`SIGNED_NAMESPACE_OP_SCHEMA_VERSION` 23, the step after core#4263's 22;
  `SIGNED_GROUP_OP_SCHEMA_VERSION` 16), so every peer of a namespace upgrades together, and the node image's
  fleet-join must be a build that answers a challenge (mero-tee).

### Added

- **A namespace ownership proof says who founded the namespace.**
  `issue-namespace-ownership-proof` now answers `founding` (founder account and
  salt) and `credential` (this node's `AccountProof<DeviceCert>` over the
  signing key) beside the signed proof. Neither is signed and neither needs to
  be: the root certifies the key, and the account and salt derive the
  namespace id. mdma needs both to accept a claim, since it cannot check the
  signer against governance; it refuses a proof from a node without them.
  Nothing is minted to produce them, and the signed payload is unchanged.

- **A relay writes an account's signed storage on its behalf.** A delegated
  run's `User`, `Shared` and `SharedMember` entries were signed by the relay
  but named the author's device, so every peer refused them. `SignatureData`
  gains `on_behalf` and `CausalActions` gains `on_behalf_accounts`; only a
  `RelayTee` may sign on an account's behalf, and only when the run actually
  writes such an entry. (breaking: the borsh layout of `SignatureData`
  changes; rebuild apps against this release) (#4366)

- **An account reads its own groups' members, subgroups, metadata and
  capabilities.** A delegated session can now `GET` a group's `members`,
  `subgroups`, `metadata` and `members/:account/capabilities`; a group outside
  its scope answers 404. Listing subgroups as an account shows Open subgroups
  and only the Restricted ones the account belongs to. (#4321)

- **Releases publish checksums, build provenance and SBOMs.** Every release
  asset ships with a SHA-256 checksum, a signed build-provenance attestation
  and an SBOM. (#4320)

- **Accounts publish presence through their relay.** An account with no node
  can now publish ephemeral presence (typing, online) through its relay,
  attributed to the account. (breaking: the presence wire format is replaced
  in place, so nodes on the old format stop exchanging presence with new ones
  until they upgrade; pairs with mero-js#230) (#4306)

- **Full-text search for apps.** An app opts in with `#[derive(app::Searchable)]`
  on a value type and `app::search_indexes!` naming the collections to index;
  its views query with `Query` (words, prefix, substring, fuzzy, keyword and
  range filters, relevance or newest/oldest-first order, snippets). Each node
  keeps a tantivy index per context in the new node-local `SearchIndex` column,
  fed by a dirty log (`SearchDirty`) staged in the same batch as every write
  and rebuilt from a scan when state moves without one (snapshot, repair,
  migration). The index is never synced or hashed, so nodes with and without
  it interoperate. Only views can call the new `search_query` host function,
  which is charged gas for the host's work; a top-20 query costs 2.0-2.8M gas
  from 2,000 to 200,000 messages, against 634M for an in-WASM scan of 10,000.
  An app without `search_indexes!` pays one export lookup. `[context.search]`
  in `config.toml` tunes or disables it (on by default). Guide:
  *Search your app's data*. (#4234)

- **Members holding `CAN_INVITE_MEMBERS` mint invitations that every peer
  accepts alike.** The inviter's permission is judged at the join's causal
  point, so a revoke concurrent with a join no longer splits replicas, and a
  recursive namespace invitation skips subgroups the inviter holds no grant in
  instead of minting ones every peer refuses. A member without a node signs
  with its bound device key. (#4243)

- **A nodeless account can link a device through a relay.**
  `POST /admin-api/namespaces/{namespace_id}/account/link-device` takes the
  account's signed device scope; the relay carries `AccountDeviceLinked` into
  the namespace endorsed with its own member key, after which the device can
  sign invitations and other delegated ops. A proof or scope that must be
  re-signed answers 400; a revoked device or unknown account answers 403.
  (#4267)

- **CPU, restart and disk metrics on `/metrics`.** `process_cpu_seconds_total`
  (linux; `rate()` of it is cores in use), `process_start_time_seconds` (a
  change means the node restarted), `storage_disk_usage_bytes{store}` (bytes
  allocated under the datastore and blobstore directories), and
  `storage_namespace_bytes{namespace_id, column}` with
  `storage_namespace_contexts{namespace_id}` (the same per-namespace RocksDB
  estimate `/admin-api/usage` reports, for the namespaces this node belongs to).
  All are sampled on the existing 30s metrics tick; the disk walk and RocksDB
  probes run on a blocking thread.
- **Fuzzy load test metrics.** vmagent scrapes every 10s instead of 30s, also
  scrapes a pinned node_exporter for the runner host (CPU, steal, memory, free
  disk), and each suite pushes `ci_test_passed`, `ci_test_exit_code` and
  `ci_test_duration_seconds` carrying the run's `commit_sha` / `branch` labels.
- **Fuzzy load test core dumps.** A node that crashes now leaves its core in
  the suite artifact (`cores/`), with the merod binary and an all-threads gdb
  backtrace; the crashed thread's backtrace is also printed in the job log.
  The runner's default core handler (systemd-coredump) dropped container cores.
- **merobox 0.6.81 in CI.** A fuzzy assertion on a failed call's output now
  fails; before, `is_set({{out}})` passed on the unresolved placeholder name,
  so fuzzy pass rates may drop where calls were silently failing.

- **`Registry<K, V, A>`** — names with at most one owner each, decided by an
  authority. Members `claim` a name into their own `Authored` entry; only the
  authority writes verdicts, and nothing is owned until one names an owner:

  ```rust
  names: Registry<String, Profile>,           // A = Tee: an attested TEE decides
  self.names.claim(name.clone(), profile)?;   // then fire the `#[app::tee]` resolver
  self.names.resolve(&name)?;                 // in the TEE; idempotent
  self.names.status(&name)?;                  // Free / Pending / Owned / Lost / Contested
  self.names.owner_of(&name)?;                // Some only once granted
  ```

  `Tee` writes through a `TeeOnly` cell, `Admin` through a `SharedStorage`
  cell whose writers are the admins (`set_admins`), and `NoAuthority` writes
  nothing and reports contests. Each verdict is its own entry keyed
  `H(name) ‖ !epoch ‖ vacant ‖ order ‖ by`, so a name's standing is the first
  one under its prefix: the highest epoch, then the lowest `order`, which is
  `H(H(name ‖ epoch ‖ owner) ‖ vacant)` — no clock, so no claim can be backdated, and
  a stale or rolled-back authority can only lose. Verdicts need no custom
  merge, so they are ordered by their keys rather than by the merge path.
  `release` marks the owner's claim and the
  authority answers with a vacancy at the next epoch. No new `CrdtType`, no new
  apply rule: the existing `TeeOnly` and `Shared` rules refuse a non-authority
  verdict, and owned ids refuse a claim forged for another account. The ABI
  describes it as a record of the claims (`AuthoredMap`) and the verdict cell.
  `calimero_storage::testing::Script` is new with it: replicas in different
  roles, deltas delivered as a test chooses, then every causal delivery order
  replayed. `apps/name-registry` is a TEE-decided username registry with a
  merobox scenario, and `apps/name-registry-admin` the same registry decided
  by admins (`Registry<String, String, Admin>`): the context's creator
  resolves, `set_admins` hands the role on, and its scenario checks on three
  nodes that a member's resolve and a rotated-out admin's resolve are refused.

- **A node behind a proxy can tell device-key callers apart
  (`server.proxy_identity`, `merod init --proxy-identity`).** Under
  `auth_mode = "proxy"` core installs no auth guard, so every caller looked
  anonymous: the caller-scoped listings (`GET /admin-api/contexts`,
  namespaces, groups) answered node-wide and `POST /contexts/:id/query`
  refused with "requires an account-authenticated session". That was harmless
  while a proxy only ever let the node owner through, and wrong for a fleet
  relay whose mero-auth serves `account_proof` logins for several tenants.
  mero-auth's `/auth/validate` now names an `account_proof` session's account
  and device in `X-Auth-Account` / `X-Auth-Device` (decided by the key record's
  `auth_method`, never guessed from `X-Auth-User`), and with the new flag merod
  reads them into the same `AuthenticatedAccount` / `AuthenticatedDevice` the
  embedded guard injects, so scoping, per-call membership checks, device
  revocation and SSE ownership behave exactly as under embedded auth. Off by
  default and ignored in embedded mode: merod cannot tell a header the proxy
  wrote from one a client did, so it is only for a node reachable through a
  proxy that replaces both on every route it authenticates and strips them on
  every other one. A present but unparseable account header is refused with
  `401 invalid_identity` rather than read as "nobody", which in proxy mode is
  the node-wide answer; the headers are removed once read, and a sealed request
  never carries them.

- **`IndexedMap<K, V>`** and **`#[derive(app::Indexed)]`** — an `UnorderedMap`
  whose value type declares secondary indexes, so the list views every app
  writes (filter by a field, count, page newest-first) are seeks instead of scans
  over the whole collection:

  ```rust
  #[derive(BorshSerialize, BorshDeserialize, AbiType, app::Mergeable, app::Indexed)]
  #[index(status_created(status, created_at))]
  pub struct Issue {
      #[index] pub status: LwwRegister<String>,
      #[index] pub labels: LwwRegister<Vec<String>>,   // one row per label
      pub created_at: LwwRegister<u64>,
  }

  issues.query("status_created").eq("open").desc().limit(20).entries()?;
  issues.query("status").eq("open").count()?;           // loads no entry
  ```

  `eq` pins an index key's next component and `range` bounds the one after, so a
  compound index answers "one status, ordered by time" in one seek; `desc` walks
  back with bounded reverse seeks (`StorageAdaptor::index_last_in`, new, backed
  by the existing `storage_index_last` host function — no new host ABI).
  `Option::None` leaves an entry out of an index and a `Vec` indexes each element.

  Nothing new crosses the wire. The indexes live in the node-local keyspace
  `SortedMap` already uses, the map reports `CrdtType::UnorderedMap` and is
  described to the ABI as one, and it serializes byte for byte as the
  `UnorderedMap` it wraps — so an app can switch an existing field to
  `IndexedMap` with no migration, and nodes on either type agree on every hash.
  Correctness does not depend on writes going through the map: a validity marker
  (the collection's `full_hash` plus a fingerprint of the index declarations)
  makes the first query after a sync, a merge or a changed declaration rebuild,
  and a write only re-stamps a marker that was current before it. That first
  query after a remote change is `O(n)`; later ones are `O(log n + k)`. A rebuild
  whose writes are dropped (node-local writes suppressed) answers by scanning
  rather than from the unbuilt index. `apps/indexed-issue-tracker` is the example,
  with a two-node merobox scenario. `apps/indexed-forum` is the harder one: a
  three-part compound index over a multi-valued field, an optional pin index,
  and `Authored<IndexedMap>` posts alongside `AuthoredSortedMap` comments and
  `UnorderedSet` votes.

- **`Guarded<C, P>`: one write policy over any keyed collection.** How a
  collection is read and who may change it are now separate choices. The
  collection `C` (`UnorderedMap`, `SortedMap` or `IndexedMap`) gives the reads;
  the policy gives the write rule, checked by every node when it applies a
  peer's write:

  ```rust
  posts:    Authored<IndexedMap<String, Post>>,          // owner edits and deletes
  messages: WriteOnce<SortedMap<String, Msg>>,           // owned, never changed
  board:    Moderated<IndexedMap<String, Post>>,         // owner, or a moderator deletes
  log:      ContentAddressed<IndexedMap<[u8; 32], Ev>>,  // keyed by content hash
  charter:  Frozen<String>,                              // one value, fixed at init
  ```

  An entry carries one stamp, so the policy is a type parameter rather than
  nesting wrappers. Reads go to `C` through `Deref`, and there is no
  `DerefMut`. `AuthoredMap` and `AuthoredSortedMap` are now aliases
  (`Authored<UnorderedMap>`, `Authored<SortedMap>`).
  - **`WriteOnce<C>`, `Moderated<C>`, `ModeratedOnce<C>`**: the `User` stamp
    gains signed `EntryRules { immutable, moderators }`, fixed at creation. An
    immutable entry refuses every changed write and every delete, its owner's
    included. A moderated entry may also be deleted by its collection's
    moderators, a writer set rotated with `set_moderators` and checked as of
    each delete. A collection returns only entries carrying its exact rules, so
    an entry written without them to dodge moderation is never read.
  - **`Frozen<T>`**: a single value whose one writer, the creating account,
    holds the new `OpMask::WRITE_ONCE` bit alone. Every node refuses a changed
    value or a removal, even one that writer signs; a byte-identical
    redelivery is accepted, so sync converges.
  - **`ContentAddressed<C>`** is the content-hash policy that was briefly
    called `Frozen<C>`; the name now belongs to `Frozen<T>`.

  `apps/indexed-forum` uses all of it: a `Frozen<String>` charter and
  `Moderated<IndexedMap>` posts moderated across two nodes in its merobox
  scenario. `apps/permissions-showcase` covers the rest, one field per
  policy, with a two-node scenario that checks each rule on the node that did
  not write. [Choosing state](docs/src/content/docs/build/choosing-state.mdx)
  is the new guide: which map, which policy, what syncs, what stays local, and
  how permissions reach nested data.

- **merod verifies a KMS that runs as a TDX cluster.** mero-kms runs as a
  frozen GCP TDX cluster, one per release, booted from a locked image whose keys
  derive from a root held only in its replicas' memory. Its RTMR3 is its image's
  own boot measurement, so merod pins it by MRTD and RTMR0-3 alone. This is now
  the only KMS merod talks to; see **Removed** for the dstack KMS path it
  replaces.

- **`AuthoredSortedMap<K, V>`** — an `AuthoredMap` with an ordered view, so a
  reader can `prefix` / `range` / `page` / `keys` instead of walking the whole
  collection. Same per-entry `StorageType::User { owner }` stamp, same
  owner-gated `update` / `remove`, same merge — and the same `CrdtType::UserStorage`
  on the wire, deliberately: the ordering is the node-local derived index
  `SortedMap` already maintains, never replicated, so two nodes holding the same
  entries with one collection each agree on the root hash. No new `CrdtType`
  variant, so no borsh discriminant change and nothing for a mixed-version peer
  to reject. The ABI gains `CrdtCollectionType::AuthoredSortedMap`, classified
  `IdentityGated` like the other authored collections; an older reader grades an
  unknown tag `Unresolvable` and so still fails closed.

  This is a **liveness** fix more than a speed one, because the two halves of
  the ownership model compound: anyone may insert under any key (insert is open
  by design), and only an entry's own owner may ever remove it. A member acting
  in bad faith can therefore grow an `AuthoredMap` without bound and nobody else
  can shrink it — not the app, not the other members — and with `entries()` the
  only iteration, every honest reader pays for that on every read, forever. The
  entries never have to be *believed* to do the damage: an app that correctly
  ignores all of them still reads all of them.

  Measured in `crates/storage/tests/read_cost_profile.rs`, fetching an 8-entry
  slice by prefix, in counted store reads:

  | entries in the collection | `AuthoredMap::entries()` | `AuthoredSortedMap::prefix()` |
  |---|---|---|
  | 250 | 500 | 17 |
  | 1,000 | 2,000 | 17 |
  | 4,000 | 8,000 | 17 |

  Linear against flat, and 2,000 further entries piled under a prefix nobody
  reads leave the slice at 17. A writer who targets your specific prefix can
  still crowd it — no collection prevents that — but an untargeted flood stops
  mattering. `AuthoredMap` keeps its no-index write cost; reach for the sorted
  one when keys are hierarchical and reads are slices of them.

- **`AuthoredMap::new()` is now generic over the storage adaptor**, matching
  `UnorderedMap` and `SortedMap`. It was pinned to `MainStorage`, so the type
  could not be instantiated with a test adaptor at all — which is why its read
  cost had never been measured. Purely a widening: `MainStorage` stays the
  default type parameter, so every existing call site is unchanged.

- **`grantedOnGroupId` on the relay descriptor** (`GET admin-api/contexts/:id/intents`),
  optional. `canAuthorOnBehalf` answers whether this node may execute a delegated
  write in the group owning this context; this says **where** the grant lives,
  which is what makes both a refusal and a later revoke actionable. Absent means
  no group reachable from here carries it, so someone must grant it — on
  `groupId`, or once on an ancestor. Equal to `groupId` means granted here.
  *Different* from `groupId` means granted on an ancestor this node inherits
  membership through — honoured there, and the case the field exists for:
  contexts routinely live in subgroups while a TEE fleet node is admitted once at
  the namespace root, so one root grant covers the fleet and this is the only way
  to see that a *root* grant is what is covering it. A revoke or a narrowing has
  to edit the group named here, not `groupId`.

  Reports where a grant *lives*, never what is *permitted*: `canAuthorOnBehalf`
  remains the only authorization answer and a client must not read this field as
  permission. Backed by `warrant_gate::authorship_grant_source`, which defers to
  `MembershipRepository::effective_capabilities` and `check_path` rather than
  re-deriving the traversal — so it cannot report a grant across a boundary the
  membership walk itself refuses to cross (a private subgroup required its own
  admission, so it requires its own grant), and cannot report one for a node
  deny-listed off an Open subgroup. Both properties are pinned by tests verified
  through mutation.

- **`GET admin-api/contexts/:context_id/intents`** — the relay descriptor a
  keyholder needs *before* minting a warrant: `executorAccount` (whom the
  warrant's `executor` must name), `canAuthorOnBehalf` (whether this node holds
  the grant on the owning group) and `groupId` (whose admin grants it).
  Both facts belong to the node, so a client could not compose them, and asking
  after the fact is too late: minting a warrant spends a nonce from a monotonic
  per-device sequence, and one naming the wrong executor is unspendable.
  `canAuthorOnBehalf: false` is an answer rather than an error — it is the
  default state of every context, since the capability is implied by neither
  membership nor admin.

- **`--can-author-on-behalf`** on `meroctl group members set-capabilities`, and
  the same row in `check-access`. The capability existed and was enforced
  everywhere, but no CLI could grant it, so delegated execution could not be
  turned on at all. Note the mask is replaced, not merged.

- **`Client::get_intent_relay`** in `calimero-client`, and `meroctl context
  intent` now uses it. The command previously took `executor` from
  `GET admin-api/identity`, which needs a credential on the relay — so on the
  relay the feature exists for, the credential-free one, it could not read it at
  all — and which does not report the grant, so it signed and spent `--nonce`
  before learning the write would be refused. It now reads both from the
  descriptor and refuses beforehand when the grant is missing, naming the group
  and the account in the `set-capabilities` command to ask an admin for.

- **`server.admin.public_intents`** (and `merod init --public-intents`): serve
  the two delegated-execution routes above without a node credential. **Off by
  default.** It opens exactly those two and nothing else, because they carry
  their own credential — the warrant commits to this context, method and
  arguments, is single-use, and is refused before execution unless the node
  holds `CAN_AUTHOR_ON_BEHALF`. A node token proves none of that, and requiring
  one makes the feature unreachable for the callers it exists for: a browser
  tab or an agent holding one signing key and no relationship with the relay.
  This is the write half of the known gap recorded in
  [direct admission](docs/src/content/docs/protocol/direct-admission.mdx);
  `/admit` is still behind the guard.

  **Breaking** for `calimero-server`: `AdminConfig::new` now takes
  `(enabled, public_intents)`. One constructor rather than a defaulting one,
  because this flag decides whether a node exposes a write path to callers
  holding no credential on it, and a convenience default is how a relay ships
  with the posture nobody intended — in either direction.

### Removed

- **`GET /auth/callback` is removed from mero-auth.** The placeholder page
  minted fake tokens and redirected to any callback URL, and nothing used it;
  meroctl's login runs its own loopback callback. (#4255)

- **The Phala / dstack KMS path. BREAKING — no compatibility shim.** The only
  KMS is mero-kms as a GCP TDX cluster (above); every upgrade brings new nodes
  and a new KMS, so nothing old has to keep working:
  - The config section `[tee.kms.phala]` is now `[tee.kms]`, with the same
    `url`, `tls.*` and `attestation.*` keys (`TeeConfig.kms: Option<KmsConfig>`,
    `TeeConfig::kms(url)`; `PhalaKmsConfig` is gone). A config that still says
    `[tee.kms.phala]` no longer configures a KMS.
  - dstack KMS verification is gone: no RTMR3 event-log replay, no compose-hash
    check, no `tee.kms.phala.attestation.allowed_compose_hashes`, no
    `kms_allowed_event_payload` in the release policy, and no `backend` /
    `kms.backend` selector. `/attest`'s `eventLog` is no longer read.
  - The signed release asset merod fetches is `kms-attestation-policy.json`
    (was `kms-phala-attestation-policy.json`); its `merod_config_path` is
    `tee.kms.attestation`.
  - The default `/attest` binding is `SHA-256("mero-kms-attest-v1")` (was
    `mero-kms-phala-attest-v1`), in lockstep with mero-kms, so this merod only
    verifies a KMS from the same generation.

- **`upgradePolicy`** from the namespace and group-info responses (`GET
  admin-api/namespaces`, `.../namespaces/:id`, `.../namespaces/for-application/:id`,
  `admin-api/groups/:id`), along with the `UPGRADE_POLICY_COMPAT` constant that
  filled it. The concept went server-side in #3393; the key survived only for
  clients that declared it required, and its own doc named the condition for
  removal — a merobox bundling a client-py built off post-merge master, with
  `MIN_MEROBOX` naming it. merobox 0.6.56 and client-py 0.6.29 satisfy both.
  It always held `"LazyOnAccess"`, so no caller can depend on its value.
  Requests are unaffected: released nodes still require the field on
  `POST admin-api/namespaces` and `POST admin-api/groups`, and both SDKs keep
  sending it ([#3485])

- **`meroctl context identity grant` / `revoke`** and the per-context
  capability request/response types behind them. The commands were shipped and
  advertised in `context --help`, but had no route, client method or handler and
  always errored. Use `meroctl group members set-caps` instead ([#3440])
- **`Client::claim_group_invitation`** and the
  `ClaimGroupInvitationApi{Request,Response,ResponseData}` types. **Breaking**
  for `calimero-client` and `calimero-server-primitives`: the method was `pub`
  on a published crate and posted to `admin-api/groups/claim-invitation`, a
  route with no handler anywhere, so any caller got a 404. Direct
  request-response join replaced the relay it belonged to ([#3450])
- **`governanceOp`** from both join responses (`POST admin-api/groups/join`,
  `POST admin-api/namespaces/:namespace_id/join`), along with
  `JoinGroupResponse::governance_op_bytes`. **Breaking** for
  `calimero-server-primitives` and for any client that decodes the response
  into a type with a required field of that name — but the node only ever sent
  `""` there, the last piece of the relay #3450 removed, so no working caller
  can depend on its value. Read `memberAccount` for the principal the join
  produced.

  Every pinned decoder was updated first, because one of them runs our own
  E2E: `mero-js` 11.2.0 and `swift-sdk` dropped the field, `calimero-client-py`
  0.6.29 rebuilt against the tolerance added in [#3530], and merobox 0.6.56
  bundles that client — so `MIN_MEROBOX` moves to 0.6.56 here, the floor at
  which a join against a node without the field still decodes ([#3485],
  [#3528])

### Fixed

- **A pending sweep no longer leaves the root hash ahead of the DAG heads.**
  When a delta's parent arrived by a path other than an inbound apply (a
  local execute, a parent restored from the database, snapshot checkpoints, or
  the startup load), the deltas waiting on it were applied without holding the
  context's execution lock through the `dag_heads` commit, so a heartbeat or
  handshake could pair their new root with the old heads. The snapshot and
  startup sweeps never committed the cascaded heads at all, leaving that pair
  torn until an unrelated delta arrived. Every sweep now holds the lock from
  its first apply through the heads commit, as an inbound apply does.

- **Sync cost, live data and execution errors are measured.**
  `sync_round_trips`, `sync_comparisons` and `sync_entities_transferred`
  (by `protocol`) are recorded once per completed initiator session from the
  totals HashComparison, LevelWise, Snapshot and DeltaSync already keep, and
  `sync_messages_sent` / `sync_bytes_sent` count every sync message at the
  transport. `storage_datastore_table_bytes{kind}` reports RocksDB's live-data
  estimate beside its live and total table-file bytes, so garbage waiting for
  compaction is visible. An execution the node could not run to completion
  is now counted in `context_runtime_execution_count_total` and
  `context_runtime_execution_duration_seconds` with `status="error"`; it used
  to skip both.

- **Heartbeats no longer advertise a root hash the node does not hold.** A
  heartbeat could pair a delta's new root with the old DAG heads (it read the
  two between the apply's two writes), and after a HashComparison responder
  or a LevelWise session the node kept advertising its pre-session root, since
  those merges bypass the executor that updates it. Both made peers count a
  same-heads / different-root divergence for a state no node was in. The
  heartbeat now reads under the context's execution lock, and every such
  session re-anchors the cached root to storage.

- **`fleet-join` on a node that is already a member answers at once.** The
  relay a namespace was founded through is its first TEE member, and when it
  was handed its own addresses as admitters `fleet-join` asked itself for
  admission and never returned; the request also held the sync loop, so
  periodic sync, namespace pulls and outbound joins stopped with it. A node
  that is already a member and holds the namespace key now answers
  `admitted=true` without attesting or asking anyone. Admitter addresses
  naming this node are dropped, opening a stream for an admission or a
  namespace pull gives up after `open_stream_timeout`, a direct admission
  runs beside the sync loop instead of inside it and is answered as refused
  after 30 s, and meroctl requests are bounded by the 30 s default request
  timeout (longer for fleet-join, joins, syncs, upgrades, installs, context
  creation, JSON-RPC calls and blob transfers). No wire change.

- **Sync safety counters count.** `sync_snapshot_blocked_total`,
  `sync_verification_failures_total`, `sync_buffer_drops_total` and
  `sync_protocol_selections_total` were registered but never incremented
  outside tests, so they read 0 whatever happened; they are now recorded where
  the event occurs. Failed syncs are recorded in `sync_duration_seconds`
  under `outcome="failure"`, so timeouts show in its tail. Eight sync families
  with no production writer (`sync_messages_sent`, `sync_bytes_sent`,
  `sync_round_trips`, `sync_entities_transferred`, `sync_merges`,
  `sync_comparisons`, `sync_phase_duration_seconds`, `sync_lww_fallback`) are
  no longer exported. (breaking for dashboards that query them)

- **`context_runtime_execution_count` is a counter.** It was a gauge that was
  only incremented, so restarts read as drops and `rate()` did not apply. It
  is now exposed as `context_runtime_execution_count_total`. (breaking for
  dashboards that query the old name)

- **The datastore's size is split by file kind.** `storage_datastore_file_bytes{kind}`
  reports SST, WAL and other RocksDB file lengths. `storage_disk_usage_bytes`
  counts allocated blocks, so on a small node it is mostly preallocated WAL
  and does not move with the data.

- **Execution latency percentiles are real numbers.** The
  `execution_duration_seconds` histogram's lowest bucket was 1s, so every
  execution landed in it and `histogram_quantile` reported a constant p95 of
  950ms whatever the latency was. Buckets now run from 0.5ms to about 16s.

- **A stuck root-hash divergence has its own counter.**
  `sync_root_hash_divergence_detected_total_total` counts every same-DAG /
  different-root heartbeat observation, transient ones included, so it climbs
  into the thousands under write load with nothing wrong.
  `sync_root_hash_divergence_escalated_total` counts only divergences that
  persisted past the heartbeat window (the ones logged as
  `DIVERGENCE DETECTED`), once per stuck hash pair; alert on that one. The
  kv-store fuzzy load test now fails if any node logs `DIVERGENCE DETECTED`.

- **A member who left or was removed from an Open group inherits nothing
  below it.** Inheritance anchored on any ancestor's row and checked the deny
  list and re-entry block of the target group only, so a member kicked from an
  Open group stayed a member of its Open children and grandchildren: it could
  join them, write in their contexts and be served their keys. A deny-list
  entry or a removal block on an Open ancestor now ends inheritance below it
  for joins, writes, key pulls, delegated standing and member lists; only an
  admin of a group above it still reaches below. An honest inherited member
  and a member an admin re-added below the removal are unaffected. The groups
  a namespace leave or a root TEE eviction closed stay closed after a root
  re-admission, until an admin re-adds the member there or it accepts a fresh
  invitation. No wire change. (breaking for mixed-version namespaces: peers on
  an older build still admit such a member's joins and writes) (#4424)

- **A member's concurrent ops that relied on a revoked capability have no
  effect.** Taking a capability away with `MemberCapabilitySet` did not void
  the member's ops that cited a cut from before the revoke, so a member could
  keep using a revoked `MANAGE_MEMBERS`, `CAN_MANAGE_METADATA`,
  `MANAGE_APPLICATION` or similar bit. An admin's revoke now voids the
  member's ops in that group that are concurrent with it and needed a revoked
  bit, and a revoke that arrives after such an op takes back the rows it
  wrote, as a removal does. Ops that needed no revoked bit, ops by a member
  who was an admin at its cut, and grants are unaffected, and the revoke
  reaches only the group it names. A metadata or application-target write a
  voided op made before the node heard of the revoke is not undone. Nothing
  signed or on the wire changes. (breaking for mixed-version namespaces: a
  node on an older build applies an op a new node voids) (#4453)

- **A nested wrapper op is refused while it is decoded.** The op inside an
  `OnBehalf` or `RootGuarded` wrapper is never itself a wrapper, and apply
  already refused one, but decoding recursed once per wrapper before any
  authentication, so a malformed payload from any peer on a namespace topic
  could abort the node. The decoder now refuses a wrapper inside a wrapper,
  for gossip, backfill, decrypted group and sealed root ops and storage reads
  alike. Encoding is unchanged and no valid op is refused. (#4446)

- **An app opens only blobs held for its running context.** `blob_open`
  returned a handle for any blob id, so an app could read bytes this node held
  for another context by naming their id. It now opens a blob only if the app
  wrote it, it was uploaded with that `context_id` on a member node, it was
  fetched from the context's peers, or it is the context's own application;
  any other blob, including one the node does not have, traps with
  `BlobNotHeldForContext`. `blob_open_in_context` still reads a blob by
  fetching it from the context's peers. The SDK's test host follows the same
  rule, so app unit tests that announce made-up blob ids now see `false`.
  (breaking: a blob written or uploaded on a node older than rc.74 is held for
  no context, so plain `blob_open` traps on it; read it with
  `blob_open_in_context`) (#4389)

- **A stub application row no longer overwrites an install.** The
  `ContextRegistered` stub, the upgrade-target stub (`TargetApplicationSet`,
  `CascadeUpgrade`) and the join bootstrap stub checked for an application row
  and wrote a size-0 stub outside the lock the install paths took, so an
  install landing in between was replaced by the stub. All of them now take
  one process-wide row lock: a stub write waits for an install of the same
  application and then leaves its row alone. (#4414)

- **A link that pushes an entity's existing subtree past the depth limit is
  refused.** The ancestor check bounded only the links an action carried, so
  moving a stored entity under a deeper parent could leave its descendants
  past 256 ancestors, and every later walk over them failed with
  `ParentChainTooLong`. An Add or Update that would do so, including a re-add
  of a deleted entity or the recreation of a deleted ancestor, is now refused
  with `ActionNotAllowed`, and sync drops it like other refused links. A
  delete also keeps listing the children that outlive it (newer than the
  delete, or `Frozen`). Honest writes never make such a move. (#4397)

- **Inbound sync streams are bounded per peer and in total.** A peer could
  open sync streams faster than they timed out and hold an unbounded number
  of them and their waiting responders. A node now holds at most 256 inbound
  sync streams, 60 from one peer, and closes a stream past either limit at
  once; the dialer's request fails and periodic sync reconciles. Blob stream
  limits are unchanged. (#4451)

- **A re-delivered gossip delta no longer replaces the copy already parked.**
  A delta that cannot be applied yet (its application is not runnable, or the
  HLC fence buffers it) is parked before its payload is checked, and the
  envelope signature does not cover the payload, so a re-delivery with
  another payload replaced the honest copy and the honest delta was lost
  until a later sync. Both parking sites now keep the first copy. A lost
  delta is still recovered by parent fetch or sync. (#4450)

- **An account that is not a member of a group cannot name a device in it.**
  An `AccountDeviceLabelled` op was checked only against a root statement or
  the device's own binding, and an account's bindings outlive its removal, so
  a removed account could keep writing device names. The named account must
  now be a member of the group at the op's causal cut; otherwise the label
  records nothing and the op still takes its place in the DAG. Labels in the
  account namespace and by current members are unaffected, and a removed
  account's descope still narrows its own device. (breaking for
  mixed-version namespaces: peers on an older build still record such a
  label) (#4425)

- **HA works for a namespace founded through a relay.** A TEE fleet node
  admitted into such a namespace never got its group key: the founding relay
  is attested in an op sealed under the namespace key, so to a node admitted
  later it was a plain member, not a trusted anchor, and the fleet node
  refused its key and `fleet-join` reported `admitted=false`. A namespace's
  founding relay is now a trusted anchor of the namespace root while it still
  holds a row there. Default invitation admitters and the invitation address
  resolver now count the founding relay as a TEE. A direct admission request
  refused as a replay because the broadcast already admitted the node now
  answers `admitted=true`. No wire change. (#4434)

- **The sealed-root replay no longer re-applies a group creation or re-stores
  a key it already applied.** Each key arrival re-fed every sealed root op in
  the log. A replayed `GroupCreated` for a group since moved was refused on
  every pass, and for one not moved it re-seated the creator as admin after a
  later removal. A replayed `KeyDelivery` stored its key again at a newer
  epoch, which could put it above a key a later rotation installed. Each
  replay also started another retry pass, so a late-joining node could spend
  tens of seconds re-walking the log and hold up governance intents. The
  replay now skips a `GroupCreated` whose group is already folded, and a
  replayed delivery of a key the node already holds is a no-op. (#4435)

- **A compacted delta's events hash and TEE trigger go with it.** DAG
  compaction deleted a delta's row but left the rows kept beside it, so disk
  still grew with delta history. They are now deleted in the same transaction
  as the row, those earlier compactions left behind are swept, and the space
  is given back. A peer asking for a compacted delta still gets "not found".

- **The gossip parent walk asks a peer for each missing ancestor once.** It
  checked each parent with linear scans, quadratic in the walk length, and
  re-requested a parent queued later in the same batch, so a DAG with merge
  edges asked for the same delta several times (79 requests for 40 ids in the
  test). The walk now keeps one set of queued ids, as the sync-manager parent
  pull already did. An id that failed once in a walk is retried on the next
  sync round. (#4421)

- **A namespace's group list shows a Restricted subgroup only to those who may
  see it.** `GET /admin-api/namespaces/{id}/groups` listed every child to any
  caller in scope, so a member learned the id and name of a Restricted
  subgroup it was not in. It now applies the filter
  `GET /groups/{id}/subgroups` already used: every Open child, and a
  Restricted child only to a namespace admin or a member of that child.
  Response shapes are unchanged. (#4374)

- **A run's unflushed storage actions no longer ship in a later run's delta.**
  JS-SDK host functions record sync actions on the host thread, and nothing
  cleared them when a run trapped, panicked or ran out of gas, so they shipped
  in the next run's delta on that thread, for whichever context ran next.
  Every execution now clears the thread's pending delta on entry and on exit.
  Rust-SDK apps were not affected, and the JS SDK's normal persist-then-flush
  path is unchanged. (#4416)

- **A member removed from an inherited Open subgroup cannot list its devices
  or seal to its members.** `GET /groups/:group_id/member-devices` and
  `POST /groups/:group_id/accounts/:account/seal` judged membership without the
  deny-list, which is where such a removal is recorded. Both now use the
  deny-list-aware check: a removed caller gets `403 NotAGroupMember`, and a
  removed target reads as absent (404) on the seal route. (#4405)

- **A key rotation that arrives before the key it is sealed under is
  applied.** Such an op was buffered, and the replay once the key arrived
  applied only the inner op, so the rotation was lost and the node kept
  sealing under the pre-rotation key, which the removed member still holds.
  The node now records the buffered rotation and applies it when the key
  arrives (by delivery, pull or the startup re-drive), including a second
  rotation sealed under the first one's key. Ops buffered by an older build
  stay as before. (#4410)

- **A join signed with a device key rotated out at the op's cut is refused.**
  The join ownership check accepted a certificate whose device had since been
  re-keyed to a later epoch, and the join applied for the account under the
  retired key. It now refuses a certificate whose device is bound to the same
  account at a later epoch at the join's cut. A join with the device's current
  certificate is unchanged. No schema version bump: an older node still
  admits such a join. (#4411)

- **Maps and sets leave out an entry filed under an id its key does not
  derive.** A peer delta can file a map or set entry at an id its key does not
  derive, which apply cannot refuse. Iteration listed it, `get`, `contains`
  and `remove` at that id returned it, and honest code could not remove it.
  `UnorderedMap`, `SortedMap`, `IndexedMap`, `UnorderedSet` and `SortedSet` now
  leave it out of every read, merge and re-key; an insert at its key refiles it
  and a remove of its key deletes it. `len` still counts it. Node-local sorted
  and indexed indexes rebuild once on first ordered read. (breaking: iteration,
  `Debug`, `PartialEq`, `Ord` and `Serialize` of these collections now need
  `K: AsRef<[u8]>`, or `V` for sets) (#4403)

- **An execution's host calls and the bytes they move are capped.** Gas meters
  wasm operators but not host calls, so a guest could loop a cheap host call,
  or one copying a large buffer, far past any honest workload. Each execution
  now allows at most 1,000,000 host calls (`max_host_calls`) and 6720 MiB of
  copied bytes (`max_host_bytes`), and traps with `HostCallLimitExceeded` or
  `HostBytesLimitExceeded` past them. The heaviest honest app measured made
  98,949 calls. The caps are deterministic and not tunable through
  `[runtime.limits]`; a deployment that raises `max_gas` more than about 10x
  could reach the call cap with an honest app. (#4408)

- **A device narrowed out of a namespace cannot keep writing state deltas.**
  Narrowing a device's scope dropped its binding but, unlike a revocation,
  recorded nothing about its signing key, so a narrowed device that had not
  folded its own narrowing could keep writing by citing older governance heads.
  The narrowing, and a link refused because the device was already revoked or
  narrowed out, now deny the key at the receive filter and on the heads path
  while no live binding speaks for it; a later widening lets it through
  again. (#4412)

- **A proxy-identity tenant subscribes only to what it is a member of.** In
  proxy auth mode with `server.proxy_identity`, the subscribe gate allowed
  every caller because no embedded guard runs, so every tenant could subscribe
  to every context and group over WS and SSE, and a membership removal never
  revoked it. A caller with an identity is now held to its membership in every
  auth mode: subscribes are filtered to its contexts and groups, admin-only
  group payloads go only to admins, and removal revokes live subscriptions.
  An identity-less caller on a no-auth node is still allowed. (#4406)

- **A remote install accepts only signed bundles, and raw wasm never runs.**
  Raw wasm derives no application id, so the join bootstrap, lazy upgrade,
  sync, blob-share and resync installs, and the relay's release resolution
  bound it under whatever id a group named, and a group target could name a
  raw blob the node held for another reason and run it. These paths now
  accept only signed bundles, every compile, ABI and migration read refuses a
  non-bundle, and `GET /admin-api/applications/{id}/abi` answers 400 for a raw
  row instead of 500. Operator installs already required a signed `.mpk`.
  (breaking: a context whose application row holds raw wasm now fails with
  "raw wasm, which never runs"; rebuild it as a signed bundle with
  `cargo mero bundle`, install it with `meroctl app install --path app.mpk`
  and upgrade the group to it) (#4413)

- **A group context runs only a release its own group named.** A node keeps one
  application row per `ApplicationId`, shared by every group on it, and any
  group admin could point that row at their own blob; a context in another
  group for the same id then ran that wasm on its own state. The row now speaks
  for a group context only when the context's group, or an ancestor, named that
  blob as its target or a rung of its upgrade ladder. Otherwise the execute,
  migrate, create, join and resync paths take the group's release, and a context
  whose release is not on the node is refused with `ApplicationNotInstalled`
  until it is fetched (the first execute's lazy upgrade fetches it).
  `migration_status` reports the row's version only when the row names the blob
  the context runs. (#4396)

- **A restarted node syncs and fetches blobs from its peers straight away.**
  merod stopped without closing its connections, so a QUIC peer kept the old
  connection open for about 10s. A node that restarted within that time
  reconnected as a second connection to a peer that already knew it, and
  gossipsub sent it none of that peer's subscriptions. The restarted node then
  logged "none subscribe to the context or namespace topic" and could not sync
  with that peer or fetch blobs from it until a message on each topic repaired
  the table, which took up to tens of seconds. Shutdown now closes every peer
  connection while the node is still running and refuses new ones, so the
  restart reaches the peer as a new connection and both sides exchange
  subscriptions. (#4407)

- **An account that administers a namespace reads its migration status through
  a relay.** The admin check on `GET /groups/:namespace_id/migration-status`
  read the node's own account, so through a relay (a `RelayTee`, never an
  admin) it refused the namespace's admin with 403. It now checks the caller:
  the node's account for a node session, the delegated account for an account
  session. A plain member is refused with the same not-admin error on both
  paths. (#4400)

- **A peer's row at the id of the next mark no longer stops a replica
  formatting.** A row a peer stored where this replica's next `RichText` mark
  would go, one that does not decode or is filed under another parent, made
  `mark` fail with a deserialization error, and a left-out row refused the
  mark outright. The mark now keeps its lamport and moves to a random replica
  (one derived from the mark itself during a migration), so it is still read
  and still wins; it fails only after `MAX_MARK_ID_ATTEMPTS` taken ids. Found
  by the `crdt_sync` fuzz target.

- **A collection no longer reads another parent's entity as its own.** A peer's
  delta could store an entity at the id one of this node's map keys derives,
  under another parent or as an orphan; `get` then returned that entity's value
  as the key's, or failed decoding it, and so did the insert that followed.
  Every collection read now treats an entity whose index names another parent
  as absent, before decoding it.

- **A peer can no longer stop a node by storing an unreadable app root.** A
  delta whose `Add` or `Update` at the root id carried bytes that do not decode
  as the app's root was stored as is, and every later call on that node
  panicked in `Root::fetch`. `Root::sync` now reads the root and its entry back
  through the decoders every call uses and refuses the delta when either fails,
  so none of its writes are stored. Found by the `crdt_sync` fuzz target.

- **The delegated routes take the intent permission, and an account reads its
  groups' upgrade state.** With `delegated_access` off, `presence-intents`,
  `context-intents` and `governance-intents` now take `context:intent` (the
  first scoped to its context, the two group routes unscoped), as `/intents`
  already did; before, they fell to the `admin` default-deny or needed
  `group:manage`. An account may read a group's upgrade, migration and cascade
  status, refused outside its own groups. (#4392)

- **`execute` checks `context:execute` on every call, in proxy mode too.** The
  method grant is checked once, in `execute_request`, for both `/jsonrpc` and
  the WebSocket, and in proxy auth mode the node reads mero-auth's
  `X-Auth-Permissions`. A token without the grant for that context and method
  is refused where it used to be answered as the node owner; a grant scoped to
  one method no longer reaches the others. (breaking for tokens that relied on
  it) (#4380)

- **A blob is announced only for a context that holds it.**
  `blob_announce_to_context` no longer records a blob for the running context:
  it announces a blob already held for that context and returns `0` (SDK
  `false`) for any other, including one missing from the node, which used to
  trap. A blob uploaded without a `context_id` is held for no context. (breaking
  for apps that announce blobs they did not upload with the context's id)
  (#4387)

- **Refused namespace ops get a backfill, and pending admission is bounded.**
  Every path into the namespace DAG (gossip, backfill, catch-up, the local
  publisher) validates and verifies an op first. A delta missing a parent is
  buffered only for a signer the namespace knows, at most 1024 per origin, and
  dropped after ten minutes; backfill batches are capped and ordered. An op
  refused for its signer is answered with one backfill from the sender per
  namespace per 30 seconds. (#4228)

- **A node actor that panics is restarted, not lost.** A panic in a handler no
  longer leaves every `Addr` to that actor closed while the node keeps running:
  the actor restarts in place and `actor_restarts_total{actor}` counts it. More
  than 5 restarts in 30 minutes exits with code 70 so the supervisor restarts
  the node. (#4391)

- **Delta history is compacted on cold and restarted contexts, and the space is
  given back.** Compaction now visits contexts with no in-memory delta store,
  counts the rows on disk rather than in memory, is not blocked by one pending
  delta, and compacts the pruned key range so the disk shrinks. (#4393)

- **Inbound blob streams are bounded.** At most 128 inbound blob and
  blob-announce streams at once, 32 per peer; a stream whose request does not
  arrive within 10 seconds is dropped. (#4326)

- **A subscription to a group this node does not hold starts no sync.** A peer
  subscribing to `group/<id>` for an unknown group no longer makes the node
  spawn a group sync and a state broadcast. (#4371)

- **A relayed call whose method failed is reported as an error.**
  `POST .../intents` answered a method that returned an error as
  `200 { returns: null }`, and `POST .../query` as an opaque `500`. Both now
  answer `400` with the error JSON-RPC `execute` gives for the same call
  (`type: "FunctionCallError"`, the message in `data`). (#4394)

- **A self-hosted relay creates a context for a member when `init` signs
  nothing.** The on-behalf check now runs after `init` and before anything
  commits or publishes, and asks for a `RelayTee` only when `init` writes an
  entry the relay signs for the author, as delegated calls already do; a
  member holding `CAN_AUTHOR_ON_BEHALF` was refused every delegated creation.
  (#4386)

- **Entry signatures are verified strictly on the node too.** The node's
  native storage environment used a lenient Ed25519 verify that accepts a
  small-order public key, under which a trivial signature verifies any
  message; it now uses `verify_strict`, as the guest host function and the
  rest of the node do. A signed `User`, `Shared` or `SharedMember` write or
  snapshot leaf under such a key is refused. (#4382)

- **The at-rest key file is owner-only from the moment it is created.** It is
  opened with mode `0600` instead of being written with the process umask and
  narrowed afterwards, and a key file of the wrong length is replaced rather
  than truncated in place. (#4297)

- **Peers judge an account's relayed writes by its membership at the cut.**
  A relayed join folds as the joiner's membership, a subgroup's creator and
  the namespace founder are Admin at the cut, and a delegated delta is judged
  as its warrant's verified account, so an account's second device counts.
  An account root can withdraw a device that never linked in the namespace.
  (#4367)

- **A cross-context call reaches only methods marked `#[app::xcall]`.** A
  module that declares no entry point, or whose xcall policy cannot be
  resolved (for example a wasm built without an embedded ABI), now refuses
  every xcall with `XCallNotPermitted` instead of accepting all of them.
  `apps/xcall-example` replaces `ping_to` with `ping_secret`. (breaking: an
  app that receives xcalls must mark each target method `#[app::xcall]`)
  (#4343)

- **Only declared event handlers run, and a delta's id covers its events.**
  A method an event names must be marked `#[app::handler]`, or peers do not
  run it; JavaScript apps run no handlers on peers until the JS SDK can
  declare them. A delta's id commits to a hash of its events, so the author's
  signature covers them. (breaking: every delta id changes and the sync
  stream protocol is `/calimero/stream/0.0.4`; upgrade every node together
  and rebuild apps that use event handlers; existing contexts catch up by
  state sync) (#4208)

- **Each sealing purpose derives its own key and binds its context.** Group
  key deliveries, TEE vault keys, app `seal_to` / `open_sealed`, account
  seals and blob transfer each use their own HKDF label and AAD, and an app
  envelope opens only in the context that sealed it. (breaking: nothing
  sealed by an earlier build opens, including stored `Sealed<T>` and
  `TeeSecret<T>` values; existing TEE namespaces must be recreated, and TEE
  node images need the same core release) (#4219)

- **A removed admin's concurrent governance ops are void.** An op whose
  signer has a concurrent removal, demotion or device revocation in the log
  is not applied, and what it added (admins, capabilities, key rotations) is
  recomputed without it, the same on every node in any arrival order. An
  admin removed while offline loses the governance edits it made offline.
  (breaking: `SIGNED_NAMESPACE_OP_SCHEMA_VERSION` 21; upgrade a namespace's
  nodes together, and clients that sign namespace ops must send 21) (#4275)

- **Blobs go only to members of their context.**
  - A blob announcement must carry a member's signed proof, and each account
    runs one announcement-triggered prefetch at a time. (breaking: the
    announce protocol is `/calimero/blob-announce/2.0.0`, so old and new
    nodes exchange no announcements, which only delays prefetch) (#4334)
  - A node serves a context's peers only blobs it holds for that context.
    Blobs stored before this release are not served to peers until uploaded
    again with a `context_id`. `blob_close` returns again. (#4239)
  - Blob provider records are verified strictly, so a record under a
    small-order key no longer verifies. (#4377)

- **Membership and device checks.**
  - A kicked or departed account no longer subscribes to, reads or lists an
    Open subgroup it inherited into. (#4373)
  - A namespace join from a revoked or narrowed-out device is refused.
    (#4370)
  - A device has its own account's standing, not that of the member who
    endorsed its link. (#4337)
  - A node attaches a device proof only to a group key it wrapped, and
    certifies only its own signing key; before, any peer could get a
    root-signed `DeviceCert` for its own key. (#4358)
  - `POST /admin-api/groups/:group_id/governance-intents` checks the
    warrant's signature, executor, founder and the node's group membership
    before it installs, fetches or records anything. (#4351)

- **Client keys, logins and tokens.**
  - A client key acts only on the contexts it was minted for, on `/jsonrpc`
    and WebSocket `execute`, and `POST /admin/client-key` takes an optional
    `application_id` that binds the key to an application. (#4193)
  - A client key is refused any `admin` or `keys` permission other than the
    exact `admin`, and a context-bound key cannot hold `admin`. (#4254)
  - Client-key tokens last 15 minutes (access) and 7 days (refresh), set by
    `jwt.client_access_token_expiry` and `jwt.client_refresh_token_expiry`
    and capped by the node-wide values. (#4259)
  - Each WebSocket `execute` message needs `context:execute` for its context
    and method. (#4316)
  - Failed logins are throttled per caller and account (5 a minute) under an
    account-wide ceiling (100 an hour), and password hashing runs off the
    async workers. (#4213)
  - The embedded auth-frontend archive is pinned by sha256. (#4253)

- **Network and server hardening.**
  - A sealed request's body is held only once its session is known, and a
    `Content-Length` over 64 MiB is refused with 413 before it is read.
    (#4317)
  - Namespace subscribers are counted from the swarm's connected peers, not
    from a table of every peer ever seen. (#4369)
  - An undecodable gossip message is logged by topic and length only.
    (#4368)

- **Storage tree walks cannot loop, and RGA text skips misplaced rows.**
  - An upsert that would link a parent loop or more than 256 ancestors is
    refused, and walks over the tree are bounded. (#4376)
  - An RGA char row keyed as the document start, or filed under a key its id
    does not derive, no longer reads back, and `insert_str_at_timestamp`
    refuses the zero timestamp. (#4378)

- **A JS app's root write is held to the storage write limits.** (#4355)

- **A remote install never rolls an application row back.** A downloaded or
  shared signed release replaces the row only at the same or a newer semver
  version, and raw wasm only fills an empty row. Operator installs through
  the admin API are unchanged. (#4354)

- **Bundle manifests are signed under a versioned domain tag.** The
  signature covers `SHA-256("calimero.bundle.manifest.v1" || 0x00 ||
  manifest)`; the bundle hash is unchanged. (breaking: bundles signed before
  must be signed again, and a registry moves with its publishers; pairs with
  app-registry#391) (#4307)

- **meroctl keeps tokens out of URLs and warns about plain http.**
  - The login callback page posts the tokens to `/callback` and clears them
    from the address bar; `GET /callback` no longer takes tokens in its query
    string. (#4318)
  - `--api` and `node add` with a plain `http://` URL to a non-loopback host
    print a warning on stderr. (#4312)

- **A write no newer than a delete no longer brings the entity back.** A
  delete and a write with the same stamp ended deleted or live depending on
  which a node received first, so replicas never converged; the delete now
  wins in either order. Entries that merge in any order (`Custom`,
  `RotationLog`, text blocks) no longer resurrect on an older write either.
  (#4349)

- **A device's namespace topic survives a superseded scope.** A node folding
  several of its own device-scope certificates at once could unfollow a
  namespace the newest scope covers and never follow it again, so the
  namespace went missing from its listing. Scope changes now apply in order,
  and a subscribe racing an unsubscribe no longer wedges the topic. (#4330)

- **A device the namespace withdrew counts as withdrawn in every group of
  it.** (#4338)

- **Runtime bounds.** Context-taking blob calls are bound to the executing
  context (#4345); JS collection writes are held to the storage write limits
  (#4347); guest memory and table maxima are enforced and guest threads are
  disabled (#4220).

- **Network and auth hardening.**
  - A namespace governance message is accepted only on its own namespace
    topic. (#4315)
  - A joiner keeps join keys only from senders it has a reason to believe.
    (#4314)
  - Root-key handoffs on credentials presented over http are capped and
    checked before signatures. (#4324)
  - Refreshing TEE evidence checks the release and the quote. (#4226)
  - Blob chunk and root ids live in separate spaces, and roots are verified
    on read. (#4221)
  - The app downloader accepts plain http registries only on localhost or a
    private network (#4305); build-utils fetches archives over https only and
    caps the download size (#4342).
  - The client redacts credentials in debug output and zeroizes the API key
    on drop. (#4322)

- **Text blocks and mark rows are kept to the bounds honest writers keep.**
  (#4230)

- **`app::bail!` can be used as an expression.** (#4333)

- **Sync repair delivers a custom-typed entry the receiver does not hold.**
  HashComparison and level-wise sync deferred every `Custom`-typed leaf to the
  in-WASM merge, which skips an entry with nothing stored locally, so such an
  entry could never reach a node that missed it. A node misses one when it
  refuses the entry's delta, for example a buffered delta applied before its
  author's binding folded there. The two nodes then held the same DAG heads
  and different root hashes indefinitely: in mero-updates, the author's read
  receipt in an `Authored<IndexedMap>`. A custom entry the receiver has no
  value for now applies as it arrives; one it holds still merges in WASM.
  (#4310)

- **A write is stamped after the version it replaces.** An overwrite, delete
  or register edit made after the wall clock stepped back could lose to the
  older value it replaced; every write's stamp is now later than the stored
  one. (#4289)

- **Sync converges for collections wider than 256 children.** HashComparison
  refused any node listing more than 256 children and the session still
  reported success, so wide collections never synced. Nodes now page their
  children (1,024 per page) and fetch the rest on demand; 2,600 children
  converge in one session. (breaking: new sync stream messages) (#4283)

- **`mero-sign` refuses a signing key file other users can read.** On unix,
  `mero-sign sign`, `cargo mero bundle --key` / `MERO_SIGN_KEY` and
  `cargo mero sign` fail when the key file grants group or others any access
  (mode `& 0o077 != 0`), naming the file, its mode and the fix (`chmod 600`).
  Keys made by `generate-key` / `cargo mero key generate` are already 0600 and
  keep working; the documented CI recipe now writes the key under `umask 077`.
  (#4250)

- **A member whose capabilities were explicitly revoked to nothing no longer
  falls back to the namespace default at the causal cut.** At-cut gates and
  the inheritance walk read a folded 0 as "nothing folded" and used the
  default, so a stripped member (the documented `CAN_JOIN_OPEN_SUBGROUPS` deny
  case) still passed gates like `require_can_create_context` and inherited
  into Open subgroups. An explicit grant, including an empty one, is now
  authoritative, and so is a group default explicitly set to nothing. No wire
  or storage change, but it changes which ops peers accept at the cut
  (breaking: old and new peers can disagree on ops from such members, so
  upgrade every peer together). (#4273)

- **TEE authorities seal the namespace TEE key only to a key a current TEE
  authority delivered.** A member holding a TEE role could supply the key the
  authorities seal to, because a key was retired only when a TEE it was
  delivered *to* stopped being an authority. `TeeVaultDelivery` now records
  its signer, and a key is retired when its signer or any recipient is not a
  current TEE authority; a retired key still opens what was sealed to it, but
  no run seals to it, and an authority creates a new key when none is left.
  The rule is applied when the key is read, not at apply, so logs stay
  identical across replicas. No schema change; TEE relays need the upgrade to
  close the hole. (#4225)

- **The guest `ed25519_verify` host function uses strict verification.** It
  now calls `verify_strict`, as core does elsewhere, so small-order public
  keys and non-canonical signatures return 0. Valid signatures from ordinary
  signers still verify. An app that branches on such crafted inputs can see a
  different result on older nodes. (#4298)

- **`meroctl` creates its config directory owner-only, and `JwtToken` debug
  output no longer prints token values.** A config directory `meroctl`
  creates, with any missing parents, is mode 0700 on Unix instead of the umask
  default; existing directories keep their mode. In `calimero-client`, `{:?}`
  of a `JwtToken` prints the access token, refresh token and metadata values as
  `<redacted>`. (#4302)

- **Replicas no longer disagree on who created a subgroup.** Two members with
  create authority could sign `GroupCreated` for the same subgroup id
  concurrently (a racer copies an id it saw gossiped before the genuine op is
  in its causal frontier). Replicas fold concurrent ops in either order, so each
  seated whichever arrived first as owner and admin and refused the other, and
  the namespace diverged for good. A subgroup id is now derived from its create,
  `created_subgroup_id(admin, parent_id, restricted, salt)` =
  `domain_hash("calimero.subgroup.id.v1", [admin, parent_id, [restricted],
  salt])`, and `RootOp::GroupCreated` carries the `salt`; apply refuses a
  create whose fields do not reproduce its id (`GroupIdNotDerived`, HTTP 400).
  Every valid create for an id then names the same creator, parent and
  visibility, so no other account can name it at all. SDKs that build a
  delegated `GroupCreated` must derive the id the same way and append the salt.
  The variant's layout changed and ships at
  **`SIGNED_NAMESPACE_OP_SCHEMA_VERSION` 20** (breaking: rc.68 and rc.69 also
  speak schema 20 but sign `GroupCreated` without the salt and apply none of
  the root-guard rules, so a mixed namespace diverges — upgrade every peer and
  relay together). (#4244)

- **Owner-level operations need the account root.** `TransferOwnership`,
  `GroupDelete` and `AdminChanged` (now owner-only) and the TEE policy ops
  travel as `GroupOp::RootGuarded` / `RootOp::RootGuarded`, carrying an
  `OwnerOpAuthorization` the account's root key signs under
  `calimero.account.owner-op.v1`. The proof names the group and a counter that
  must exceed the group's stored `GroupOwnerOpCounter`, so it cannot be
  replayed, and an older root is accepted only while its rotation chain reaches
  the group's epoch. A lost device key therefore cannot hand a group away,
  delete it or swap its admins. A node that holds the root signs the proof
  itself; a nodeless account sends it as `rootProof` to
  `transfer-ownership`, `owner-delete`, `namespaces/{id}/admin` and the TEE
  policy endpoints (breaking: unguarded forms of these ops are refused).
  (#4244)

- **Search snippets mark a half-typed word.** A prefix query (`Query::prefix`,
  the as-you-type mode) returned an empty snippet whenever its only word was
  the one being typed, and left that word unmarked otherwise: tantivy cannot
  name the words a prefix automaton matched. The snippet now marks the words
  the last one completes to (up to 64) alongside the finished ones.

- **Counting, membership tests and removals on guarded collections no longer
  load every child.** `len` / `keyed_len` on `AuthoredVector`, authored,
  write-once and moderated maps and `UserStorage` read a node-local count row,
  and `contains` / removal read one child, so these calls cost the same number
  of reads at any size. At 10,000 messages `send_message` goes from 2.96G gas
  (28,825 reads) to 2.27G (513 reads) and `get_message_count` from 346M to
  17.8M; a chat channel reaches the default 1e9 budget at about 4,350 messages
  instead of 3,150. No format change. (#4232)

- **A relay can read a subgroup it is seated in.** A relay that created a
  subgroup or founded a namespace for a member was refused member listing and
  group info (`node is not a member of group`): its seat was in the
  membership rows but not in the governance state peers derive from ops. It is
  now, through a new `OpPayload::RelaySeated` (appended; existing tags
  unchanged). (#4245)

- **`merod run` and `merod kms probe` refuse a KMS nothing verifies.** In a
  build without `mock-attestation`, a `[tee.kms]` with no named release
  (`MERO_TEE_VERSION`, `MERO_KMS_VERSION` or `MERO_KMS_RELEASE_TAG`) and no
  enabled config allowlists (`enabled = true`, `accept_mock = false`) is
  refused before any request, as `init` already did. A deployment with
  `enabled = false` and no release now needs one or the other. `kms probe`
  also verifies against the named release's policy when there is one.

- **Profiling image: merod no longer segfaults under jemalloc heap profiling.**
  jemalloc backtraced sampled allocations with libunwind, which cannot see the
  unwind tables wasmer registers for JIT code (`__register_frame`) and crashed
  walking an allocation made under a wasm call. It now uses libgcc's unwinder,
  which does; the image build fails if configure picks anything else.

- **An attested TEE can no longer be moved out of the TEE roles.** An admin
  could `MemberRoleSet` a `ReadOnlyTee` / `RelayTee` row to `Member`, `ReadOnly`
  or `Admin` (or reach the same through a `MemberAdded` upsert, or name the TEE
  in `AdminChanged`), leaving an enclave's key with ordinary authorship in its
  own name and outside every `is_tee()` check. A TEE row now only moves to the
  other TEE role, under the admission policy's mode; anything else is refused
  at apply and before signing with `TeeMemberRoleLocked` (HTTP 403). Removing a
  TEE is unchanged. **`SIGNED_NAMESPACE_OP_SCHEMA_VERSION` is now 13**: a v12
  peer still applies the demotion, so v12 and v13 nodes cannot share a
  namespace — upgrade every peer together.
- **A refusal names a group by its hex id.** Errors from the context and
  governance handlers printed `ContextGroupId` with its derived `Debug`, 32
  decimal bytes a caller could not paste back into a request. It now has a
  `Display` (lowercase hex, the form the admin API uses) that refusals use, and
  its `Debug` is `ContextGroupId("<hex>")`.

- **An SSE subscriber no longer misses a live delta published right after it
  subscribes.** An SSE connection joined the node-event broadcast on its event
  task's first poll rather than when `GET /sse` returned, so an event emitted
  in between never reached it; an ephemeral presence delta lost that way is
  never re-sent, because an unchanged heartbeat produces no diff. The broadcast
  is now joined before the task is spawned, making the `/sse/subscription`
  acknowledgment a real readiness signal. The ephemeral-presence e2e's SSE
  client (`subscribeSse`) also resolved as soon as the stream's headers
  arrived, before it had even sent the subscribe, so the scenario published
  into an unsubscribed session and intermittently failed "SSE subscriber
  received node 1 presence over gossip"; it now waits for the acknowledgment
  and checks the context is in it.

- **A TEE admitted by another TEE gets its key.** Both the owner and an
  admitted TEE vouch for a fleet-join announce; when the TEE's admission
  reached the owner first, the owner answered `AlreadyMember` and published
  nothing. The joiner, which bootstraps by pull, then never saw an owner-signed
  gossip op, so it recognised no peer as an anchor and refused the key from the
  owner itself: admitted in governance, keyless, never following a context
  (`tee-cards-late-tee`, flaky). A key-recovery response now counts as an
  anchor's when the envelope is signed by an anchor's key, checked before the
  gate, so a responder that only claims an anchor's identity is still refused.

- **An `#[app::mergeable]` value in a signed entry converges however two writes
  of it arrive.** Apply dropped a signed `User`, `Shared` or `SharedMember`
  write whose nonce was below the stored one before the merge ran, so for an
  entry the app merges by its own rule (a map of `#[app::mergeable]` values in a
  `SharedStorage`/`TeeOnly` cell, or in an `Authored` map written from two
  devices of one account), the node that received the newer write first kept it
  and the node that received it last merged: under a "keep the lower" rule one
  read 9 and the other 3, on different root hashes. Such a write now reaches the
  merge, as it already did in a `Public` entry, for an entry whose STORED type
  merges whatever the order (`Custom`, and on applied bytes `FugueTextBlock`)
  and only when the write names that same type, since the type is not signed.
  Every signature, writer-set, mask and owner check still runs first and
  refuses as before, and the merge is idempotent, so a replayed older write
  changes nothing it has not already folded in. The stored `updated_at` no
  longer moves back when an older write is merged. Every other entry keeps the
  stale skip. The `Shared` arm takes the same rule, but no `Shared` anchor
  carries such a type, so nothing there changes. Pinned by
  `crates/storage/tests/converge_signed_mergeable.rs` (both delivery orders via
  `testing::Script`, plus re-delivery) and
  `crates/storage/src/tests/interface.rs`'s `stale_write_to_a_merging_entry`
  (a non-writer's stale write is still refused, a relabelled one is still
  skipped, a merged one leaves the stamp at the newer write). Mixed versions: an older node still diverges on such an entry
  until it upgrades; no stored format changes.

- **A collection inside a TEE-only cell reaches members.** Merge refused every
  entity at a TEE-only id but the cell's `Shared` wrapper and its
  `SharedMember`s, and a collection's own entity there is `Public`. So a peer
  dropped it, and every entry beneath it with it, whose ancestor it is, by
  delta and by HashComparison alike: a `Registry<_, _, Tee>` field's TEE wrote
  verdicts no member took, members kept the name `Pending`, and the TEE logged
  `HashComparison sync did not converge` while members logged `an entity at a
  TEE-only id must be part of the TEE's own cell` for each push. A collection
  id beneath a TEE-only parent now carries a tag of its own
  (`is_tee_only_collection_id`), where merge takes a `Public` entity, as it
  does at a `SharedStorage` cell's collection id; everything else at a TEE-only
  id is refused as before, a `Public` entity at an entry id included. A
  collection a TEE already stored at the old id keeps it (the cell's value
  names it) and still reaches no member. Only a `TeeOnly` given its id from a
  field name has TEE-only ids: a `Registry` field's verdicts (unreleased), or
  one built with `new_with_field_name`; the state macro does not reassign a
  bare `TeeOnly` field, which lives at a cell id, so `tee-dice` and
  `tee-cards` were never affected. Pinned by
  `a_state_field_s_verdict_reaches_members_by_delta_and_by_repair` in
  `tests/converge_registry.rs` and
  `a_tee_only_collection_id_takes_only_the_collection_s_own_entity`.
  `testing::Script::push` repairs one replica from another as HashComparison
  does, and `Script::run` keeps the author's signatures, as a node does.

- **A read-only replica applies the deltas it receives.** A `ReadOnly` or
  `ReadOnlyTee` member merge-applied every inbound state delta and then threw
  the result away: the apply runs `__calimero_sync_next` with the local node as
  executor, and the execute path's non-member gate (B3, #2382) asked whether
  THAT node may author state, which a read-only member may not. Its DAG still
  recorded the delta as applied, so it served stale state until the heartbeat
  logged `Divergence detected (same DAG heads, different root)` and a snapshot
  repaired it — a TEE resolved `name-registry-contested` over a state with no
  claims. The delta applier now calls `ContextClient::apply_remote_delta`,
  which marks the run `WriteSource::RemoteDelta`; for it the gate asks only
  whether this node replicates the context (any role), since the receive path
  already verified and authorized the delta's author. Every other run is
  `WriteSource::Local` and gated as before: a read-only node's own writes are
  still discarded, including a `__calimero_sync_next` named over JSON-RPC, and
  a node with no role still discards what it applies. Pinned by
  `handlers::execute::state_write_gate_tests` in `calimero-context`.

- **`TestHost` runs `init` as the harness account.** The build closure ran
  under the storage layer's default account while every `call` and `view` ran
  under the SDK's, so whatever `init` recorded as its creator (the first admin of
  an `AccessControl`, a `Moderated` collection's first moderator, a `Frozen`
  value's writer) was not the account the test then called as. Apps worked
  around it by rotating moderators or wrapping `init` by hand; those workarounds
  are no longer needed. Pinned by `permissions-showcase`'s
  `the_harness_account_founds_the_space_and_moderates_it`.

- **Known issue, reproduced: two owners claiming one key split a context.** An
  owned entry's id comes from its key alone, and every node refuses a write that
  would change an entry's owner, so each node keeps whichever claim of a key
  reached it first. Two accounts inserting one key, or one member delivering a
  claim of a taken key to a joiner first, leave nodes holding different entries
  for good. `crates/storage/src/tests/owned_collisions.rs` reproduces both as
  ignored tests; the fix, deriving an owned entry's id from its owner as well as
  its key, follows separately. Until then, key owned entries so that two
  accounts can never choose the same key.

- **A collection nested inside a guarded entry is now guarded too.** A
  `UnorderedMap` field inside an `Authored` post was stored as its own `Public`
  entries, so any member could add, change or delete them regardless of who
  owned the post, and a `Frozen`/`ContentAddressed` value's nested collection
  could be rewritten. A nested collection now inherits the enclosing entry's
  domain at any depth: its writes carry the enclosing owner's stamp, other
  members' writes are refused locally, entries that stamp does not admit are
  never read, and nested writes inside an immutable entry are refused. Pinned by
  `crates/storage/src/tests/nested_domains.rs`.

  **Breaking:** the `User` stamp's borsh layout gains `rules`, and `OpMask`
  gains a bit; state written by an earlier build does not decode. No
  migration is provided.

- **Quotes from a debug TD are refused.** A TD launched with
  `TDATTRIBUTES.DEBUG` reports the same MRTD and RTMRs as the production TD it
  came from, but its host can read and write its memory, so no measurement
  allowlist could tell them apart. `verify_attestation` now reports
  `quote_verified == false` for a debug TD, so `is_valid()` and `policy_valid()`
  refuse it everywhere they are used: TEE admission, merod's KMS check, mero-kms
  key release and `calimero-tee-verify`. Production GCP TDs set only
  `SEPT_VE_DISABLE`; a test pins that the real quote fixture is not flagged.

- **The storage cost gate no longer fails at random on `vector_get_nth`.**
  `Vector::get` walks the whole child trie, whose shape follows the entries'
  random ids, so identical runs read 35 to 43 rows at `n=10`. With 7 samples,
  one declared tolerance could not satisfy both halves of `reproducible.rs`: an
  18% band failed as too wide on a 3% spread and as too narrow on a 23% one. The
  workload now draws its ids from a fixed seed through the new native-only
  `env::with_seeded_random_bytes`, so its counts are exact and its tolerance is
  0 like every other deterministic workload.

- **merod refuses a KMS that is not running a released compose file**
  (mero-tee#338). Node keys are derived from the KMS's dstack *app* key, so
  anything running under that app can derive them, and the app owner can upgrade
  the app to another compose file whose registers may still be allowlisted.
  merod now replays the `eventLog` that `/attest` already returned, recomputing
  every RTMR3 event digest from its contents, and trusts the `compose-hash` event
  only if the replay reproduces the quote's RTMR3. The hash must be in the signed
  release policy's `kms_allowed_event_payload`, and a policy without one is now
  refused. Every published policy carries it (the 2.3.69 fixture does). A
  config-only deployment can set the new
  `tee.kms.phala.attestation.allowed_compose_hashes` (or the external policy's
  `kms_allowed_event_payload`); when unset, merod warns and skips the check, so
  existing configs keep working. **A mero-kms without `eventLog` in `/attest` is
  refused on the release-policy path.**

- **A snapshot whose applied state does not hash to the claimed boundary now
  fails for retry** instead of being published anyway. The receiver recomputed
  the root from the state that landed, compared it to the boundary the sender
  promised, and on a mismatch logged a warning and stored the computed hash —
  leaving the node advertising a root whose state it does not hold. That claim
  satisfies every root-hash equality check, including the one that selects a
  repair protocol, so the only node that knew its state was incomplete had just
  told the rest there was nothing to repair. Forward-compatibility declines are
  unaffected: a future-schema or signature-rejected *leaf* does not move this
  hash, and shows up as the per-page `snapshot page applied with rejections`
  warning ([#3607])

- **Snapshot sync no longer serves state its announced boundary does not
  describe.** Both boundary checks on the serve path — before page generation
  and the recheck after it — compared `ContextMeta.root_hash`, but a local
  execution commits context state well before it persists the new root hash
  into `ContextMeta`. For that window the metadata still reported the pre-write
  hash while every entity the snapshot reads had already moved, so both checks
  passed and the receiver was handed post-boundary state under a pre-boundary
  hash. It then adopted the mismatching hash and advertised a root whose state
  it did not hold, which satisfies every root-hash equality check — including
  the one that selects a repair protocol — so nothing ever corrected it. Both
  checks now read the root through the context's ROOT `Index` entry, which is
  written in the same batch as the entities it covers ([#3595])

### Changed

- **Tombstone GC sweeps every 10 minutes, and the interval is configurable.**
  The new `[gc] check_interval` (seconds, default `600`) replaces the fixed
  one-hour cadence, so a tombstone every member has applied goes within about
  one to two intervals instead of up to two hours. When a tombstone may go is
  unchanged: only after every member device has applied the delete. A sweep
  reads every state row, so `gc_sweep_duration_seconds`, `gc_rows_scanned`,
  `gc_tombstones_collected` and `gc_sweeps` now show what it costs; lengthen
  the interval on a node where sweeps take a meaningful share of it.

- **Per-message signer and membership checks are point reads.** Every
  readiness beacon, ack, migration heartbeat and blob announce resolved its
  signing key to an account by building the namespace's live bindings, even
  for a stranger's freshly signed message, and checked membership by listing
  every member. A new `GroupSignerDevice` index (store prefix `0x58`; the
  ledger pointer moves to `0x59`), kept in step with every binding write,
  rotation, revocation, narrowing and teardown, and point membership reads
  make each check flat in namespace size: a key lookup at 1,000 devices takes
  about 21 µs instead of 18 ms. (breaking: there is no backfill, so in a store
  written before this release a signing key resolves to no account until its
  device is relinked) (#4447)

- **A storage write re-reads fewer rows.** A local write now hands along the
  rows it has just read or written instead of reading them again, and a link
  descends the parent's trie once instead of three times. An `UnorderedMap`
  insert at 1,000 entries reads 11 rows instead of 21, and an update 9
  instead of 13. Rows written, stored bytes and root hashes are unchanged;
  host-call counts drop, identically on every node. The storage index lock
  now spans from the read to the write. (#4432)

- **The DAG's pending set no longer scans once per delta.** Cleaning up or
  evicting pending deltas that wait on one missing parent, and evicting an
  origin's oldest pending delta at its cap, each scanned the pending set once
  per delta, which a peer could drive to quadratic time. Both are now indexed:
  an insert from an origin at its cap behind 8,000 older deltas takes 1.8 µs
  instead of 762 µs. Waiters one apply unblocks on the same parent now
  cascade in id order rather than arrival order. (#4439)

- **Storage writes and delta applies read and write fewer rows.** A write
  walked its ancestors twice and rewrote every level above even when the hash
  had not moved; the walk now runs once and stops at the first parent that
  already holds the hash. Applying a peer's delta walked once per action, so a
  shared ancestor was rewritten once per entity beneath it; a delta's walks now
  run as one pass that writes each ancestor once. A map insert reads 41% and
  writes 18% fewer rows, and a delta updating one map writes 84% fewer (kv set
  139 µs instead of 199). Stored bytes and root hashes are unchanged; host-call
  counts drop, identically on every node. (#4401)

- **Storage: about 27% less CPU per call.** The row codec hashes each row's data
  once per read and hands index records to callers decoded instead of through a
  borsh round trip, and a data-only read skips the hash checks it does not
  need. A kv set takes 146 µs instead of 201, and a chat send 165 µs instead of
  227 (`tools/storage-compare/RESULTS.md`). No format, row-count or host-call
  change.

- **A tombstone is collected only once every member device has applied the
  delete.** The 24-hour retention is gone: on every heartbeat each node sends
  a signed `StateBeacon` with its DAG heads and root hash, and GC waits until
  every other member device has reported the heads and root that hold the
  delete. A silent member keeps the context's tombstones on disk until it
  catches up or is removed; the default GC interval is 1 hour (was 12).
  (breaking: `StateBeacon` is a new gossip message that older nodes drop and
  never send, so their peers keep every tombstone until they upgrade) (#4361)

- **A register that is a whole `SortedMap` value is stored without its
  stamp.** This saves 16 B per register-valued entry, as #4340 did for
  `UnorderedMap`; `Vector` elements keep their stamp. (breaking: guest storage
  format, no migration; rebuild apps against this release) (#4359)

- **Storage: fewer reads, smaller deltas and tombstones.** (breaking: no
  migration; upgrade every node and rebuild every app against this release
  together)
  - An entity's index and data are read in one row read: half the rows read
    on lookups, and Fugue text inserts cost about 41 reads per character at
    10,000 characters instead of 62. No format change. (#4335)
  - An Update to an entity the receiver already holds ships no parent
    ancestor: a kv-store overwrite's delta goes from 162 to 130 B, a chat
    message edit from 297 to 265 B. An update the receiver cannot place is
    dropped and repaired by sync instead of failing the whole delta. (#4339)
  - A register that is a whole `UnorderedMap` value is stored without its
    16-byte stamp, which nothing read: a kv-store entry goes from 237 to
    221 B. (#4340)
  - A tombstone row stores no hash and no `deleted_at` equal to its
    `updated_at`: 94 to 54 B per deleted map entry. (#4341)

- **auth-frontend v1.3.6 is embedded.** (#4296)

- **Storage: smaller rows, fewer reads, smaller deltas.** (breaking: no
  migration; upgrade every node and rebuild every app against this release
  together)
  - Keyed owned entries leave their 32-byte id out of the row, as other map
    entries already do: an `AuthoredMap` row goes from 144 to 112 B per entry.
    (#4280)
  - An entity's index and data are written in one row write instead of a read,
    modify and write per part: inserts read 27% fewer rows and write 17% fewer.
    No format change. (#4285)
  - Delta ancestor chains leave out the implied context root, and the ancestor
    count is a `u8`: a map insert's action goes from 168 to 133 B. (#4282)
  - Events are stored as borsh instead of JSON, and GC compacts a context's
    state slice after reclaiming enough of it, so deleted rows give their disk
    space back. (#4283)
  - RocksDB state compression is tuned: on-disk state is 2.7% smaller for kv
    and 7% for chat, at ~1.2x slower cold point reads. (#4286)

- **Tighter default CSP.** The auth pages allow only their own scripts and one
  hashed inline script, and connect only to their own origin, the app
  registry and local development hosts; the admin dashboard gains
  `script-src 'self'`. (#4256)

- **A pairing statement expires.** The statement a device signs in `pair-init`
  carries its signing time, and `pair-complete` refuses one older than 5
  minutes or more than 60 seconds in the future. (breaking: the statement is
  144 hex characters and the signing domain is `calimero.device.pairing.v2`)
  (#4299)

- **Node GC also drops collected tombstones from their parent's
  `deleted_children`, so a delete no longer costs bytes forever.** Each delete
  left 32 B in the parent row, rewritten by every later write to the
  collection; in one measured case 10,000 deletes left 320,563 B after GC, and
  now leave 559 B. An id is dropped only once its tombstone is collected under
  the existing 1-day retention. GC now scans lock-free and reclaims each context
  under its execution lock. No on-disk or wire change. (#4281)

- **Storage: one row per entity, keyed by tag and id; state another 22–35%
  smaller.** (breaking: no migration; upgrade every node and rebuild every app
  against this release together) An entity's index record and data share one
  row, whose `own_hash` is derived from the data and whose trailing element id
  is not stored twice; the index's optional fields share one flags byte. State
  keys are the 33 bytes `tag ‖ id` instead of a hash, so entity rows no longer
  store their id, and `#[app::private]` blobs are keyed `0xFF ‖ Sha256(key)`.
  The HLC writer id is 8 bytes instead of 16, which changes every stored
  timestamp, the sync wire and the signed delta preimages. Child-trie bucket
  slots keep only each child's id and hash. Measured on the same probe as
  #4210: kv state per entry 428 → 278 B, chat state per message 1,240 →
  966 B. State written by earlier versions is not readable by this one, and
  wasm built against an earlier SDK cannot run on it. (#4266)

- **Tighter input validation across sync, auth, governance and bundles.**
  (breaking: upgrade a namespace's nodes together) Sync responders serve only
  the context a stream's `Init` authenticated, authorless rows and tombstones
  apply only from a peer that may write the context, and the DAG heads reply
  proves the identity the responder serves as; the sync and blob protocol ids
  move to `0.0.3`, and `DagHeadsResponse` and `BlobAuthPayload` gain fields, so
  older nodes cannot sync or read private blobs with this one. Governance ops
  are judged by stricter rules (`GroupCreated`, `TransferOwnership`,
  `ContextRegistered`, `Noop`, `MemberAdded`, `GroupDeleted`,
  `GroupReparented`, capabilities at a cut). mero-auth and calimero-server
  enforce per-route admin permissions under `NODE_PATH_PREFIX`, and
  `[server.cors]` is now parsed and applied: a browser app that opens a
  WebSocket from another origin must list it in `allowed_origins`. (#4203)

- **Storage: state 35–78% smaller and deltas 64–71% smaller, in a new stored,
  hashed and wire format.** (breaking: no migration; upgrade every node and
  rebuild every app against this release together) A parent's child trie is
  one bucket row up to 16 children and a 16-way node above that; nested
  collections are written on their first insert; `LwwRegister` drops its 32 B
  `node_id`; an index row stores `full_hash` only when it is not derivable, and
  `Metadata` has a compact flags-first encoding; deltas stop re-shipping an
  unchanged context root and app-state entry, and carry ancestors as ids only
  (the delta-id preimage changes with them). Measured on kv-store and mero-chat:
  kv state per entry 662 → 428 B, kv delta per set 788 → 286 B, chat state per
  message 5,658 → 1,241 B, chat delta per message 3,910 → 1,118 B, chat rows
  per message 38 → 5. State and deltas written by earlier versions are not
  readable by this one. merodb reads the new layout, walking children through
  the child trie. (#4210, #4240)

- **A TEE is admitted as a replica or as a relay, and a replica never relays.**
  (breaking: upgrade a namespace's peers together) The namespace's TEE
  admission policy gains a `mode`: `replica` (the default, and what every
  policy set before it reads as) admits `ReadOnlyTee`, which replicates,
  anchors sync and availability and may author as the TEE authority, but is
  refused as the executor of a delegated write whatever capability it holds;
  `relay` admits the new `RelayTee`, which also authors members' writes under
  their warrants by its role, with no `CAN_AUTHOR_ON_BEHALF`. Before, any TEE
  admitted under a namespace's default mask held that bit and relayed by
  accident. `warrant_gate::executor_standing` decides it for `POST
  .../intents`, the relay descriptor and every peer at the cut; a role refusal
  is a `403` up front (`ExecuteError::DelegatedWriteRefused`), never a `200`
  for a write the node then drops. Setting the mode converts the TEEs already
  admitted (`MemberRoleSet` per direct row). Over the admin API it is `mode`
  on `PUT/GET .../settings/tee-admission-policy`; in meroctl, `group settings
  set-tee-admission-policy --mode relay` and `get-tee-admission-policy`, both
  new. New ops `TeeAdmissionPolicySetV2` / `TeeReleaseAdmissionPolicySetV2` and
  the `RelayTee` role are appended, so no stored discriminant moves;
  `SIGNED_NAMESPACE_OP_SCHEMA_VERSION` is 12, so nodes before and after cannot
  share a namespace, and a client must sign at 12 too: mero-js 22.1.0 or later
  (older releases are refused with `schema version mismatch: expected 12, got
  11`). See [TEE attestation](docs/src/content/docs/protocol/tee-attestation.mdx)
  and [delegated authorship](docs/src/content/docs/protocol/delegated-authorship.mdx).

- **Core's example apps follow the [securing-state](docs/src/content/docs/build/guides/securing-state.mdx)
  rules.** Records a member must own are the member's own entries
  (`indexed-forum` votes, `blobs` file records and `private_data` game hashes
  in an `AuthoredMap`, under ids naming their author and read with `get_by`),
  ownership is by account rather than device (`env::account_id()`), and
  the owner `blobs`, `collaborative-editor` and `fugue-editor` record at init
  is a `Frozen<String>`. `private_data`'s
  `add_secret` now returns the game id it minted. Reads that listed every
  owner's row (`authored-sorted-kv-store`'s `my_notes`) count only rows the
  caller holds.

- **Owned collections have per-owner keys.** (breaking: fresh contexts only)
  Every owned entry (`Authored`, `WriteOnce`, `Moderated`, `ModeratedOnce`,
  `UserStorage`, `AuthoredVector`, and every collection nested in an owned entry)
  now lives at an id derived from its key AND its owner, and every node refuses
  an owned entry at any other id and any other entity at an owner-derived id.
  Two accounts inserting one key hold two entries on every node, whatever order
  the writes arrive in; before, each node kept whichever claim reached it first
  and the context split for good (a member could split a joiner on purpose).

  The key-only methods (`insert`, `get`, `contains`, `update`, `modify`,
  `remove`, `owner_of`, `owned_by_me`, `entry_schema_version`) act on the
  caller's own entry. New: `get_by`, `contains_by`, `entry_schema_version_by`,
  `entries_with_owners`, `entries_by`, `my_entries`, `entries_at` and, on
  moderated collections, `remove_by(&owner, &key)` — a moderator's `remove(key)`
  now removes only its own entry. `entries`, `len` and the ordered reads span
  every owner (one key appears once per owner, ordered by key then entry id). A
  name unique across everyone needs `ContentAddressed` or moderation.
  `migrate_my_entries()` walks `my_entries()`. `SIGNED_NAMESPACE_OP_SCHEMA_VERSION`
  is 9, so nodes before and after cannot share a namespace. No borsh layout
  changes; the entry layout test is unchanged.

- **Owned entries settle on one id, one key and one write on every node, and
  nodes must be upgraded together.** (breaking: fresh contexts only)
  `SIGNED_NAMESPACE_OP_SCHEMA_VERSION` is 11, so nodes before and after cannot
  share a namespace.
  - *An owned map's entry holds the key its id derives.* A map's or
    `UserStorage`'s owned entry lives at a keyed owned id and its bytes end in
    `u32_le(key length)`, so every node checks the key on apply, in snapshot
    verification and on the local write path, and refuses an entry holding
    another key. Before, a patched owner could file key B under key A's slot:
    reads skipped it and `len` counted it. `len` is now exact. A key's
    `as_ref()` bytes must be the tail of its borsh encoding (`String`,
    `Vec<u8>`, `[u8; N]`, account ids); a custom key whose bytes are not is
    refused on insert. Reads of owned maps no longer bound `K: AsRef<[u8]>`:
    the owning wrapper binds the key bytes once.
  - *An owned collection held in a `SharedStorage` value (or any
    `WriterSetCell`) now syncs.* Its entries live at an id bound to their owner
    and to the cell jointly, and every node takes one only from its owner while
    the owner is one of the cell's writers. Before, the author's node stored the
    entry and every peer refused it. An owned map there is bound to its cell,
    its owner and its key at once, so its `len` is exact too, and an owned entry
    in a cell at any other kind of owned id is refused.
    `Interface::verify_snapshot_entity_signature(id, parent, data, metadata)`
    takes the parent the snapshot record names, which both checks need.
  - *A written-once entry settles on its owner's earliest write.* Of every
    authentic write the owner makes to a `WriteOnce`/`ModeratedOnce` key, every
    node keeps the one with the lowest `(signed nonce, content hash)`, so two
    devices writing one key before seeing each other no longer split the
    context. Deleting one is terminal: no write lands on that owner's key again,
    in any order, a delete that arrives before the entry is kept as a seal, and
    the node's tombstone GC never collects either record.
  - *A buffered snapshot leaf gets the page apply's verdict.* A `Shared` or
    `SharedMember` leaf a node buffered as future-schema is held, when drained,
    to its signer's place in the writer set and to its anchor's rotation log, as
    the page apply holds it. Before, a `Shared` leaf was checked only for its
    signature, so a member serving a snapshot could plant one it may not write,
    and a `SharedMember` was deleted unapplied. A leaf whose signature fails is
    refused, and one the store can never decide is dropped after a bounded
    number of drains, so every buffered entity settles.

- **The admin API answers a refused request with a 4xx or 503, not 500.** Governance,
  upgrade, join, TEE-policy, ownership-proof, group-creation, application-update,
  device-label, resync and blob-upload refusals used to reach the API as untyped
  errors, which it answers as `500 Internal server error`. They are now typed, and
  answer with the status that says what the caller should do (#4151, #4152, #4154,
  #4156, and the device-label, context-seed and resync refusals):

  | Status | Means | Examples |
  | --- | --- | --- |
  | `400` | Fix the request | an invalid invitation, TEE policy, ownership-proof field or `bytecode_id`; a blob that doesn't match its `?hash=` |
  | `403` | Not allowed; only a grant helps | this node is not a member or a direct admin; a nested subgroup without namespace admin; a caller this node can't act as |
  | `404` | Not held here | an unknown group or namespace; a `bytecode_id` blob not on this node |
  | `409` | Conflicts with current state | an upgrade the gate refuses (the message says why); an expired invitation; a taken group id; a device this node can't name now; a resync without `force` |
  | `413` | Too large | a blob upload over 1 GiB |
  | `503` | Retry later | a join whose group key hasn't arrived yet |

  Response bodies keep their messages; only the status changes. Real node
  failures, such as store reads, signing and migrations, still answer `500`. A client
  that treated every failure as a server fault, or that branches on status, should
  read these as refusals: retrying a `400`, `403` or `409` unchanged will not help.

- **An authorship grant now reaches wherever membership reaches, and nodes must
  be upgraded together.** `CAN_AUTHOR_ON_BEHALF` is resolved by the delegated-write
  gate on the group owning the context and, failing that, on that group's
  membership *anchor* — the ancestor the relay inherits its membership through.
  It previously read the row on the owning group and nothing else.

  This is the shape a relay fleet actually has. A TEE node is admitted once at
  the namespace root while contexts live in subgroups (channels, DMs, per-team
  groups), and a capability is not copied down the tree — so a namespace-wide
  grant authorized nothing, and there was no row to write for an inherited member
  short of admitting the relay directly to every subgroup. `grantedOnGroupId`
  reported the root grant while the gate refused it, which was the descriptor
  telling a caller where the grant was and the node then turning it away.

  Two directions, and both are authorization evaluated at the cut:

  - **Widening.** An ancestor grant now counts, for an *inherited* member. Bounded
    by membership: a `Restricted` subgroup is still a wall (it required its own
    admission, so it requires its own grant), a node deny-listed off an `Open`
    subgroup is still refused there, and a **direct** member of the target does
    not reach the ancestor at all — a group that admitted the node in its own
    right decides for itself. The fleet path is the inherited one: a TEE
    admission into an `Open` subgroup runs through `admit_member_if_absent`,
    which writes no row for a node that already inherits. The cost, stated
    rather than hidden: a root grant also reaches `Open` subgroups created
    *after* it. The scoping did not disappear, it moved to where the admin
    writes the grant.
  - **Narrowing.** A bare capability row with no membership behind it no longer
    authorizes. No writer produces that state deliberately — `MemberCapabilitySet`
    bails unless the account is already a direct member, and `remove_member`
    deletes the capability row with the member row — but those deletes are not
    atomic, so it is reachable, and the gate's answer is now one deny-list-aware
    predicate instead of two that can drift.

  **Upgrade together.** A node running this and a node running the old read
  disagree about whether the same delegated delta is authorized, and would then
  hold different state. There is no wire-format change and no migration; the
  divergence is in the verdict.

  Determinism is unchanged, and for a stronger reason than the owning-group read
  gave: resolving the membership already reads the anchor's own state —
  `check_path` consults `CAN_JOIN_OPEN_SUBGROUPS` in exactly the capability row
  this bit lives in. A peer that cannot read the anchor cannot resolve the
  membership either, and refuses the delta a step earlier.

  Pinned by tests verified through mutation: restoring the old one-row read fails
  the widening case, the deny-list revocation and the narrowing case, and leaves
  every other test in the file green. The `Restricted`-boundary test passes either
  way, which is the point — it is the invariant the widening must not break.

- **Storage wire formats changed in six ways, and nodes should be upgraded
  together.** App-defined merge now actually runs for a custom type stored in a
  collection — it previously resolved last-write-wins with the app's rule never
  called — and getting there moved several stored and transmitted formats:
  `CrdtType`'s borsh tags to `0x80+` ([#3743]), `Custom(String)` to a
  `Custom(CustomTypeId)` digest at a new tag ([#3789]), map entries to
  value-first ([#3796]), and entries gained a `crdt_type` stamp ([#3799]).
  `Message::sequence_id` and `Init::pop` were already lockstep before this.

  Five of the six announce themselves: they move a discriminant, so a
  pre-upgrade node hits an unknown tag and fails the decode. **The map-entry
  reorder does not.** `Entry<(K, V)>` became `Entry<(V, K)>` — the same fields,
  the same total length, no tag — so a stale reader takes the value bytes for
  the key and carries on. Depending on the key type that surfaces as a decode
  error or as plausible nonsense, and neither is reliable.

  There is no in-band detection: the handshake versioning that would have
  refused an incompatible peer was never wired to the live path and has been
  removed rather than left looking functional ([#3811]); [#3810] tracks building
  the negotiation CIP §2.3 specifies. Until then the upgrade order is a
  convention, not something the code enforces.

  Low stakes while the network is alpha and upgrades are coordinated — noted so
  the constraint is written down somewhere before that stops being true

- **`POST admin-api/namespaces/:namespace_id/join`** names the id it returns
  `namespaceId`, not `groupId`. The endpoint shared its response DTO with
  `POST admin-api/groups/join`, so it leaked the internal noun - a namespace is
  a root group underneath, and `POST admin-api/namespaces` was the only
  namespace endpoint that translated that on the way out. A client reading the
  namespace endpoints could not use one spelling across them; merobox's
  `join_namespace` step has declared a `namespaceId` export all along, which
  silently captured nothing. `groups/join` is unchanged and still returns
  `groupId`. **Breaking**: a client deserializes into the DTO rather than
  reading the JSON loosely, so one built before this rejects the response until
  it is rebuilt - `calimero-client-py` tracks core's master and needs no source
  change, only a release, and `calimero-client`'s `join_namespace` now returns
  `JoinNamespaceApiResponse`

- **`governanceOp` on both join responses is now optional on deserialization.**
  The node still sends it (always `""`), so nothing changes on the wire; the
  `serde` default is what lets a client compiled from this point survive the
  field's removal. `calimero-client-py` builds `calimero-server-primitives`
  from master and ships as a prebuilt wheel inside merobox, so without this a
  removal breaks every E2E scenario that joins a namespace, on a client nobody
  has rebuilt. Prerequisite for [#3485] ([#3530])

## [0.11.0-rc.10] - 2026-07-05

> **Draft — release manager to curate.** Another large hardening release
> (~95 PRs since rc.9): DoS bounds across node/server/network, IDOR and
> auth fixes in the server API, CRDT/storage correctness, a meroctl
> security batch, and hot-path performance work. The `Fixed` set is
> representative rather than exhaustive — please trim/expand to house
> style before publishing.

### Added

- **Configurable CORS origin allowlist** and private-network toggle on the
  server ([#3153])
- **Optional path confinement** for `install-dev-application` ([#3155])

### Changed

- Hot-path performance: WS node events serialized once and fanned out as
  shared bytes ([#3185]); confirmed CRDT/DAG hot-path costs removed
  ([#3187]); delta-store restore streams payloads instead of holding them
  all in RAM ([#3181])
- Type-safety refactors: typed id newtypes for key/app/namespace/group ids
  ([#3105]), `GroupCapabilities` bitflags ([#3093]), encapsulation pass
  ([#3087])

### Fixed

- **Server API** — context membership enforced on WS and SSE subscribe
  (subscription IDOR) ([#3134]); blob delete/announce scoped to
  owned/installed blobs ([#3132], [#3133]); private blobs no longer cached
  publicly ([#3131]); `GET /admin-api/contexts` paginated and list page
  sizes clamped ([#3135], [#3138]); blob upload body capped ([#3174]); raw
  errors no longer leak in 500s ([#3170]); duplicate alias create returns
  409 ([#3169])
- **DoS bounds** — decoded governance-op fields, nonce windows, the scope
  projection op-log, the peer-identity cache, peer-supplied snapshot
  page/byte limits, namespace governance-op collection, and parent-fetch
  walks are all bounded ([#3178], [#3176], [#3180], [#3177], [#3172],
  [#3173], [#3171]); per-execution runtime budgets for storage, blobs,
  returns & events ([#3143]); local xcall cascade bounded ([#3156]); URL
  application install gets a timeout + body cap ([#3152])
- **Network** — discovery peer book capped against sybil growth ([#3179]);
  autonat server-support set pruned on disconnect ([#3175]); blob-provider
  DHT records authenticated with a signature ([#3168]); inbound handshake
  identity bound to the transport with a proof of possession ([#3167]);
  libp2p resource-exhaustion & DHT hardening ([#3109])
- **Storage / CRDT** — collection soundness: aliasing UB,
  corruption-masking, read-only writeback ([#3166]); add-wins merges no
  longer resurrect deleted entries ([#3084]); tombstone lifted when a
  strictly-newer write outlives it ([#3123]); child tombstone cleared on
  re-add ([#3085]); single-pass, capped tombstone GC ([#3100]);
  overflow/truncation guards ([#3165]); key-encoding & panic-path
  hardening ([#3148])
- **Governance** — member capabilities cleared on removal so re-add can't
  restore stale privileges ([#3189]); `resolve()` off-by-one depth bound
  ([#3188]); genesis founder counted as "another admin" in the last-admin
  guard ([#2983]); invitation signature verified before seeding namespace
  admin ([#3147]); key delivery, rotation & namespace-boundary hardening
  ([#3114])
- **Node lifecycle** — graceful shutdown, real health/ready endpoints, and
  store durability hardening ([#3115]); persisted-but-unapplied deltas
  re-driven on restart ([#3124]); peer-claimed snapshot root not trusted
  on local hash error ([#3150]); ReadOnly write-gate fails closed on
  lookup error ([#3145]); `update_application` caller authorized
  ([#3127])
- **meroctl** — secrets resolved from env/file/stdin instead of argv
  ([#3070]); loopback OAuth callback bound to a single-use state nonce
  ([#3071]); least-privilege token scope per command ([#3072]);
  destructive commands confirm (TTY prompt or `--yes`) ([#3074]); plus
  output, parsing & dispatch fixes ([#3073], [#3075], [#3076], [#3077],
  [#3078])
- **Runtime / client** — host-function safety hardening: UB, OOM,
  transmute, block_on ([#3097]); wasm guest validation & blob refcounting
  ([#3128]); integer-cast & clock-arithmetic hardening ([#3125]); client
  HTTP/auth layer hardened against replay, herd, traversal & OOM
  ([#3142]); reverse-proxy base path preserved for token refresh & auth
  probe ([#3149])

### CI

- Supply-chain & image hardening: action pins, image digests, checksums,
  secret handling ([#3104]); prebuilt base images pinned by digest
  ([#3158]); advisory/CVE scanning enabled and lock-fixable advisories
  cleared ([#3091])

## [0.11.0-rc.9] - 2026-06-30

> **Draft — release manager to curate.** A large security & correctness
> hardening release (~90 PRs since rc.8). The notable items are grouped below;
> the `Fixed` set is representative rather than exhaustive — please trim/expand
> to house style before publishing.

### Added

- **Node-enforced xcall caller policy** — `#[app::xcall(from_same_app)]`
  restricts a cross-context entry point to callers running the same
  application id, enforced by the node so authorization no longer relies on
  every target hand-checking `env::xcall_origin()` ([#3068])
- **Method-aware contract-gate route coverage** with nest resolution ([#2960])

### Changed

- Hot-path performance: removed redundant reads, serializations, and
  allocations ([#3055])
- Storage internals: `reject_frozen` bool replaced with a `RemoveMode` enum,
  and the opaque `HlcDriftError` replaced with a typed `ClockUpdateError`
  ([#3052], [#3020])

### Fixed

- **Auth / server** — default-deny admin API, SSE session ownership, and
  permission-update escalation closed ([#3040]); node-bound tokens fail closed
  with no request host ([#3063]); auth-guard permission enforcement and
  revoked-token 403 mapping ([#3018], [#3066]); node home/datastore restricted
  to owner-only ([#3045]); secret/logging hygiene ([#3064])
- **Storage / CRDT correctness** — deleting a node now tombstones its whole
  subtree so GC can reclaim it ([#3061]); RGA/CRDT HLC receive-path,
  tombstone-aware merge, deterministic id-seed, and RekeyTarget compile-guard
  ([#2950]); unified equal-HLC delete-vs-update tiebreak and non-regressing
  tombstone nonce ([#3029], [#3036]); Frozen child deletion rejected, undefined
  OpMask bits rejected ([#3039], [#3046])
- **Network** — connection limits + startup dial-burst cap ([#3065]);
  per-peer address cap and blob-download size cap ([#3013], [#3014]);
  backpressure on a full event channel ([#3027]); cold cross-network
  namespace-join wait ([#3059])
- **Runtime sandbox limits** — bounded precompiled-module deserialization,
  commit-artifact, and pending-delta-map sizes ([#3016], [#3009], [#3010])
- **Governance** — typed op discriminants with bounds checks ([#3048]);
  locally-recomputed cascade-delete on `GroupDeleted` ([#3043]); admin identity
  moved to the new owner on `TransferOwnership` ([#3034])
- **Store / crypto** — version+nonce bound as AAD in AES-GCM ([#3004]);
  state-delta root hash sealed inside the encrypted payload ([#3051]);
  mandatory expected app hash in TEE attestation ([#2980]); `PrivateKey` raw
  access gated and zeroized ([#3041])

## [0.11.0-rc.8] - 2026-06-26

### Added

- **SDK↔core wire-contract gate** — a CI gate that pins the SDK-to-core wire contract, catching unintended changes to the cross-boundary surface before they ship ([#2895])

### Changed

- **Op-level `state_hash` field removed** (BREAKING, flag-day) — governance ops no longer carry a per-op `state_hash`; the op schema version is bumped. This is a one-shot flag-day change with no mixed-version compatibility window — all nodes in a context must upgrade together ([#2946])
- **Unified op-store cutover (C2/C3)** — locally-authored governance and group ops are now persisted to, and the context projection is reconstructed from, a single unified op-store rather than the governance-DAG fold. Rolled out as an observe-only completeness gate → read shadow → read-flip with atomic dual-writes, then removal of the now-redundant legacy dual-writes. Internal storage refactor; no public API change ([#2911], [#2912], [#2915], [#2916], [#2918], [#2922], [#2923], [#2924], [#2925], [#2927], [#2933])
- **Centralised post-sync governance reconciliation (P6)** — governance convergence is unified across all sync backends: a single `scope_root` convergence verdict, a pre-sync divergence check when entities otherwise agree, and a `GovDiverged` verdict that pulls governance during sync ([#2935], [#2936], [#2941], [#2944])

### Fixed

- **Namespace founder derivation** — the namespace founder is now derived from a replayable genesis op instead of inferred from later state, fixing replicas (notably TEE replicas) that could seed the wrong founder admin and reject the owner's governance DAG ([#2931], [#2474])
- **Governance local-apply resilience** — a `state_hash` mismatch on a local group apply now warns instead of bailing, so a transient divergence no longer aborts the apply ([#2939])

## [0.11.0-rc.7] - 2026-06-24

### Removed

- **NEAR wallet authentication removed** (BREAKING) — the `near_wallet` auth provider, `NearWalletConfig`, the `near-crypto`/`near-jsonrpc-client`/`near-primitives` dependencies, and the `NEAR_API_KEY` env var are gone. The `WalletType`/`NearNetworkId` types and the wallet add-public-key wire structs (`Payload`, `WalletMetadata`, `SignatureMetadataEnum`, `AddPublicKeyRequest`, …) are removed; `RootKey`/`ClientKey` no longer carry a `wallet` field. Principals whose only root key used NEAR wallet must re-register via another provider (e.g. user/password). NEAR remains supported only as an opaque context-config protocol label (untyped, node-ignored).

### Added

- **`SortedMap<K, V>` collection** — a key-ordered map for range queries, prefix scans, and pagination ([#2559])
  - `range(a..b)`, `prefix("user:")`, `page(offset, limit)`, ascending `entries`/`keys`/`values`, and `first`/`last`
  - Backed by a node-local, non-synced ordered secondary index (RocksDB `SortedIndex` column): `range`/`prefix` are `O(log n + k)` seeks and `page` is `O(limit)`. Adaptors without an ordered keyspace (e.g. `PrivateStorage`) transparently fall back to an in-memory sort
  - Same add-wins CRDT merge as `UnorderedMap` — the index is a derived view of the synced entry set, not synced itself, so it adds no merge path and self-heals after a sync via a `full_hash` validity marker
  - New `apps/sorted-kv-store` example; `SortedMap` ABI marker added to the conformance apps
- **`SortedSet<T>` collection** — the `BTreeSet` to `UnorderedSet`'s `HashSet`: an ordered set with `range`/`prefix`/`page`/`first`/`last`, same add-wins union merge and the same on-disk ordered index as `SortedMap` ([#2559])
- **In-process unit-test harness (`calimero_sdk::testing::TestHost`)** - Exercise app logic as ordinary Rust under `cargo test` — no WASM build, no node, no merobox. `TestHost::new(MyApp::init)` runs methods via `call`/`view` against an in-memory mock host that records `app::emit!` events and `app::log!` lines and serves a configurable executor identity (`call_as` for multi-author CRDT tests). The `#[app::state]` macro generates the storage bridge; apps opt in with `calimero-storage`'s `testing` feature as a dev-dependency. All core example apps now ship `#[cfg(test)]` tests using it ([#2551])
- **Local mock-TEE fleet test harness** — `merod run --mock-tee` (dev/test-only, off by default, refuses to start with a real KMS attestation) makes a node produce and accept a synthetic mock attestation quote through the real `fleet-join`/announce/admit path, so the TEE/HA fleet lifecycle can be exercised locally with no TDX hardware. Plus `calimero_tee_attestation::generate_mock_attestation` (additive) ([#2855])

### Fixed

- **TEE replica replication correctness** — three governance/TEE bugs surfaced by the new mock-TEE harness ([#2855]):
  - a buffered encrypted `ContextRegistered` is now re-driven after its subgroup's `GroupCreated` applies (and via a curative startup sweep), instead of being stranded when the post-`KeyDelivery` retry hit `group not found for state hash computation` (#2848)
  - `fleet-join`'s first announce into an empty gossipsub mesh no longer returns HTTP 500 — it falls through to the re-announce loop (#2491)
  - a TEE replica whose namespace root is bootstrapped via the KeyDelivery seed now seeds `CAN_JOIN_OPEN_SUBGROUPS`, so it can replicate Open subgroups via inheritance
- **Born-Open atomic subgroup create** — `RootOp::GroupCreated` carries visibility, so an Open subgroup is created in one op (no Restricted-then-flip window) and no longer leaves a transient direct `ReadOnlyTee` row (#2771). Wire-breaking change to `GroupCreated`

## [0.11.0-rc.6] - 2026-06-19

### Added

- **SDK compile-time misuse diagnostics** — common app-authoring mistakes (including guarded-storage misuse) now surface as compile-time errors, and the macro lint suite is revived as a CI gate ([#2795])
- **SDK capability-trait diagnostics** — clearer compile-time errors for collection key types and at the RPC boundary ([#2801])
- **Richer host-boundary panic messages** — panics crossing the app/host boundary now carry more actionable detail ([#2805])

### Fixed

- **Fleet TEE nodes replicate Open subgroups, and TEE eviction is namespace-wide** — a root-admitted `ReadOnlyTee` now auto-follows (replicates) the contexts of Open subgroups it inherits, instead of only being authorized for them. And a namespace-root `MemberRemoved` of a `ReadOnlyTee` now cascades through every descendant subgroup (including Restricted subgroups created by other members), so a namespace owner can evict a fleet TEE node namespace-wide with a single root removal. The cascade is scoped to `ReadOnlyTee`; normal-member Restricted-subgroup membership autonomy is unchanged ([#2809])
- **`#[app::view]` resolves in downstream crates** — registered as a no-op marker so it no longer fails to resolve when used through a dependency ([#2799])
- **Blob-aware same-id update skip** — the context update path now correctly skips a redundant blob update when the blob id is unchanged ([#2796])
- **`LwwRegister::value_mut` drop-stamping guard** — guards against an incorrect last-writer timestamp being stamped on mutable-borrow drop ([#2806])

## [0.11.0-rc.5] - 2026-06-18

### Added

- **Transparent TEE admission into Restricted subgroups** — an entitled fleet TEE node admitted at the namespace root is now automatically admitted (as `ReadOnlyTee`) into the namespace's Restricted subgroups and delivered their per-group keys, so it serves reads across the whole namespace without per-subgroup configuration. Open subgroups need no admission (covered by inherited membership + the namespace key) ([#2772])

### Fixed

- **Governance op-events now emit after the op-log entry is persisted** — events are collected during apply and flushed only after the op-log append (both the group-op and namespace RootOp paths), closing a race where a subscriber reacting to an event could read the op-log back before it was written. Replays of an already-logged op no longer re-emit ([#2792])

### Security

- **Leave/eviction now deletes group encryption keys (forward secrecy)** — self-purge on a `ReadOnlyTee` removal, and namespace/subgroup deletion, now delete the AES group encryption keys (`GroupKeyEntry`) in addition to the per-member signing keys, so an evicted fleet replica retains no decryption material for the groups it left ([#2776])

## [0.10.1-rc.10] - 2026-03-30

### Added

- **TEE admission policy governance op** - Group admins can set a `TeeAdmissionPolicySet` governance op defining allowed TDX measurements (MRTD, RTMR0-3, TCB statuses) for attestation-based auto-admission of fleet TEE nodes ([#2086])
- **Attestation-based group admission** - Fleet TEE nodes announce their TDX attestation on the group gossip topic; existing peers verify via DCAP and auto-admit valid nodes through a `MemberJoinedViaTeeAttestation` governance op ([#2086])
- **Admin API endpoint** - `PUT /groups/:group_id/settings/tee-admission-policy` to configure TEE admission policy ([#2086])
- **`TeeAttestationAnnounce` gossip message** - New broadcast message type for fleet TEE node attestation announcements ([#2086])
- **Quote replay protection** - `is_quote_hash_used()` prevents the same TDX quote from being used to admit multiple identities ([#2086])
- **Public key binding** - Announced public key is cryptographically bound to the TDX quote via `report_data[32..64]` ([#2086])

## [0.10.1-rc.9] - 2026-03-25

### Added

- **Local group governance** replaces all blockchain/NEAR/relayer infrastructure for context management. Group membership, context registration, and member capabilities are now managed via signed gossip ops and a local `group_store`. The external client, proxy client, proposal system, relayer crate, and all NEAR protocol dependencies have been removed from core.
- **`merod init`** now produces a local-only config; the `--group-governance` flag and relayer signer are no longer applicable.
- **`calimero-context-config`** has been simplified — the `client`, `client-base`, and `near_client` Cargo features and associated NEAR/relayer transport code have been removed.
- **JWT authentication via query parameters** - Server now accepts JWT tokens passed as `?token=` query parameter in addition to the `Authorization` header ([#2079])
  - Enables WebSocket and EventSource connections from browser clients (which cannot set custom headers)
  - Header takes precedence — if `Authorization` is present it is validated exclusively, never falling back to the query param
  - Applies to both embedded auth service and server-level auth middleware

### Fixed

- **meroctl auth flow** - Improved reliability and UX of the browser-based authentication flow ([#2068])
  - Reduced auth timeout from 300s to 120s
  - Clearer messaging: informs users they have 2 minutes to complete sign-in in the browser

## [0.10.1-rc.8] - 2026-03-24

### Added

- **Automatic nested CRDT support** - Applications can now use natural nested structures without state divergence
  - `LwwRegister<T>` - Last-Write-Wins register for any value with timestamp-based conflict resolution
  - `Mergeable` trait - Universal merge interface for all CRDT types
  - Automatic merge code generation via `#[app::state]` macro
  - Global merge registry for runtime type dispatch
  - Runtime integration - WASM modules auto-register merge functions on load
  - Supports unlimited nesting depth: `Map<K, Map<K2, Map<K3, V>>>` works
  - Zero developer burden - no registration code, no merge calls needed
  - Backward compatible - existing apps work unchanged
  
### Fixed

- **RGA insert_str position bug** - Text was appending to end instead of inserting at specified position
  - Fixed tie-breaking logic in `get_ordered_chars()` to sort by descending timestamp
  - Ensures sequential mid-document insertions work correctly
  - Added regression test to prevent future breakage

### Documentation

- Added comprehensive nested CRDT documentation
  - User guide: `crates/storage/NESTED_CRDTS.md`
  - Architecture docs: `NESTED_CRDT_SOLUTION_COMPLETE.md`
  - Performance analysis: `WHEN_MERGE_IS_CALLED.md`
  - Implementation guides for future enhancements

## [0.8.0] - 2025-01-07

- Introduced comprehensive blob storage system with runtime API, peer-to-peer
  discovery, and CLI support. ([#1319], [#1337], [#1340], [#1342], [#1422],
  [#1361])
  - Blobs can be shared and discovered across peer nodes.
  - Full integration with `meroctl` and `merod` commands.
  - Support for blob deletion.
- Standalone authentication service with JWT-based authentication. ([#1336],
  [#1385], [#1470], [#1360])
  - Username/password authentication provider.
  - Support for multiple nodes from a single auth server.
  - WebSocket authentication support.
  - Mock JWT token generation endpoint for development.
- Automatic ABI emission from application code. ([#1392], [#1498], [#1415])
  - Semantic ABI emission with optimized type collection.
  - Released standalone ABI extraction tool (`mero-abi`).
- Private application data storage with `#[app::private]` macro. ([#1504])
  - Encrypted storage utilities for sensitive application data.
- `cargo-mero` CLI build tool for Calimero applications. ([#1317], [#1512])
- Migrated client functionality to separate crate for better modularity.
  ([#1432])
  - Python client bindings (moved to separate repository). ([#1436], [#1440])
- Implemented Prometheus metrics for network and context execution. ([#1429])
- Application watch command for monitoring app changes. ([#1476])
- Application uninstall command. ([#1408], [#1349])
- Implemented append-only log for state deltas. ([#1345])
- Stabilized delta sync mechanism. ([#1352], [#1389], [#1390])
- On-demand context sync. ([#1371])
- Decouple relayer as separate component. ([#1489])
- Decouple blockchain primitives from server. ([#1449])
- Deprecated Stellar integration. ([#1480])
- Workspace-wide version management. ([#1444])
- Bumped libp2p to latest version and Rust to 1.88.0. ([#1423])
- Server initialization improvements. ([#1305])
- Removed feature flags; enabled admin/jsonrpc/websocket unconditionally.
  ([#1522])
- Request/response debug logging in server. ([#1475])
- Added `is-authed` endpoint for authentication status. ([#1383])
- Added `NEAR_API_KEY` environment variable support. ([#1406])
- Made localhost:2528 default server if not specified. ([#1388])
- Removed interactive CLI from node crate. ([#1426])
- Pass aggregates by reference for improved WASM ABI compatibility. ([#1356])
- Fixed memory explosion in WASM execution. ([#1405])
- Fixed runtime pointer handling issues. ([#1459])
- Fixed forced init not removing old database. ([#1368])
- Improved auth header validation during JWT verification. ([#1471])
- Fixed broadcast-triggered sync exclusivity. ([#1390])
- Fixed server body truncation logging. ([#1510])
- Fixed application installation during initial context sync. ([#1344])
- Fixed context application updates. ([#1366])
- Major runtime logic module refactoring and documentation improvements.
  ([#1495], [#1497])
- Added comprehensive runtime unit tests for host functions. ([#1474])
- Multiple Docker setup fixes. ([#1359], [#1346])
- Build warning cleanup. ([#1514], [#1492], [#1519])

## [0.7.0] - 2025-06-13

- Massive rework of the core to the actor model. ([#1263], [#1132], [#1158],
  [#1232], [#1246], [#1238], [#1251])
  - The node can now handle requests to multiple contexts in parallel.
  - Node sync is now much more robust.
- Applications now compile once on first use, and are cached for subsequent
  invocations. ([#1291], [#1280]; thanks [@onyedikachi-david])
  - This leads to a x10±8 performance improvement in request execution.
- `meroctl` now supports remote node management. ([#1237]; thanks [@Nathy-bajo])
- Introduce alias listing to `meroctl`. ([#1276]; thanks [@cy4n1d3-p1x3l])
- Constrain `PrivateKey` exposure, protect it from being printed in logs, copied
  or sent over the wire. ([#1256]; thanks [@onyedikachi-david])
  - This also means context join no longer requires a private key, just the
    invitation payload.
- Introduce context config permission management to the API, web ui and CLI.
  ([#1233]; thanks [@onyedikachi-david], [#1240]; thanks [@Nathy-bajo])
- The CLIs now report when there is an available version update. ([#1226];
  thanks [@cy4n1d3-p1x3l])
- Introduce context proxy proposal management to the CLIs. ([#1285]; thanks
  [@rtb-12])
- `--version` output in `meroctl` and `merod` now includes some build info like
  git status and rustc version. ([#1257]; thanks [@dotandev])
- Nodes now advertise their public address, and TLS has been removed from the
  server. ([#1254])
- Replace all blocking operations with async equivalents. ([#1266]; thanks
  [@dotandev])
- Decouple rocksdb from calimero-store ([#1245]; thanks [@dotandev])
- Simplify `meroctl` connection handling significantly which makes it more
  robust and maintainable. ([#1293]; thanks [@Nathy-bajo])
- Fixed `context identity ls` crash when no default context is set. ([#1241])
- Remove only-peers, visited and gen-ext apps ([#1261], [#1270])
- Remove `node-ui` from the repo, fetching a pre-built release from it's own
  repository. ([#1268])
- Fix all docker image issues. ([#1294], [#1295], [#1296], [#1297])

## [0.6.0] - 2025-05-05

- Introduced default alias selection with the `use` command for contexts and
  identities. ([#1171]; thanks [@rtb-12])
- Introduced alias substitution in call arguments. ([#1223]; thanks
  [@Nathy-bajo])
- Support alias creation on context invitation and joining. ([#1181], [#1151];
  thanks [@cy4n1d3-p1x3l], [@iamgoeldhruv])
- Introduced event-triggered command execution with context watch. ([#1224];
  thanks [@Nathy-bajo])
- Permit running nodes without server authentication. ([#1174])
- Enabled forced alias creation and validation for safer configuration.
  ([#1227], [#1180]; thanks [@rtb-12])
- Introduced Dockerfile for meroctl. ([#1214])
- Improve the login experience in the webui. ([#1209])
- Added a way to launch the webui from the interactive CLI. ([#1205]; thanks
  [@iamgoeldhruv])
- Remove a redundant config field from the merod config. ([#1206]; thanks
  [@Nathy-bajo])

## [0.5.0] - 2025-03-27

- Added Ethereum integration
- Decoupled contracts from core repository
- Extended e2e tests to include proxy contract functionalities
- Added autonat protocol

## [0.4.0] - 2025-02-18

- Added support for aliases which can replace hash based IDs
- Minor fixes on admin dashboard
- Optimized e2e tests
- Optimized release process
- Unified release artifacts into single Github Release
- Extracted install scripts to `install-sh` repository

## [0.3.1] - 2025-01-29

- Fixed get application endpoint and the corresponding meroctl command

## [0.3.0] - 2025-01-16

- Introduced ICP integrations, achieving full feature parity with NEAR and
  Starknet
- Improved replay protection on external interactions, fixing spurious failures
  from expired requests
- Moved protocol selection to context creation, and out of the config
- Allowed the specification of all protocol's default context configuration
- Exposed, and enabled functionality for context proxy storage
- Introduced bootstrap command for quick development as a demo
- Added additional REST endpoints for easier access and information retrieval

## [0.2.0] - 2024-12-05

Rust SDK:

- env::executor_id() for fetching the runtime identity (no arbitrary signing,
  however).
- env::context_id() for fetching the context ID.
- calimero_storage::collections::{Unordered{Map,Set},Vector} for conflict-free
  operations
- Self::external() for external (blockchain) operations

Node:

- Removed the coordinator
- All messages sent between peers are now end-to-end encrypted
- Peers can share the application blob between one another, in case one of them
  doesn't have it installed
- The node has been split up into 2 binaries
  - merod retains node-specific commands, init, run, config
  - meroctl hosts client commands like context create, etc..
- merod config now has a generic & more flexible interface
- query & mutate in the API have now been merged into just execute
- interactive CLI now uses clap, making it more robust (merod)
- Added --output-format json for machine-readable output (meroctl)

Integrations:

- NEAR: expanded implementation to include a deployment of a proxy contract for
  every created context, which facilitates context representation on the network
- Starknet: reached feature parity with the NEAR implementation, allowing
  contexts to be created in association with the Starknet protocol.

<!-- versions -->

[unreleased]: https://github.com/calimero-network/core/compare/0.11.0-rc.10...HEAD
[0.11.0-rc.10]: https://github.com/calimero-network/core/compare/0.11.0-rc.9...0.11.0-rc.10
[0.11.0-rc.9]: https://github.com/calimero-network/core/compare/0.11.0-rc.8...0.11.0-rc.9
[0.11.0-rc.8]: https://github.com/calimero-network/core/compare/0.11.0-rc.7...0.11.0-rc.8
[0.11.0-rc.7]: https://github.com/calimero-network/core/compare/0.11.0-rc.6...0.11.0-rc.7
[0.11.0-rc.6]: https://github.com/calimero-network/core/compare/0.11.0-rc.5...0.11.0-rc.6
[0.11.0-rc.5]: https://github.com/calimero-network/core/compare/0.11.0-rc.4...0.11.0-rc.5
[0.8.0]: https://github.com/calimero-network/core/compare/0.7.0...0.8.0
[0.7.0]: https://github.com/calimero-network/core/compare/0.6.0...0.7.0
[0.6.0]: https://github.com/calimero-network/core/compare/0.5.0...0.6.0
[0.5.0]: https://github.com/calimero-network/core/compare/0.4.0...0.5.0
[0.4.0]: https://github.com/calimero-network/core/compare/merod-0.3.1...0.4.0
[0.3.1]: https://github.com/calimero-network/core/compare/merod-0.3.0...merod-0.3.1
[0.3.0]: https://github.com/calimero-network/core/compare/merod-0.2.0...merod-0.3.0
[0.2.0]: https://github.com/calimero-network/core/releases/tag/merod-0.2.0

<!-- contributors -->

[@rtb-12]: https://github.com/rtb-12
[@cy4n1d3-p1x3l]: https://github.com/cy4n1d3-p1x3l
[@iamgoeldhruv]: https://github.com/iamgoeldhruv
[@Nathy-bajo]: https://github.com/Nathy-bajo
[@dotandev]: https://github.com/dotandev
[@onyedikachi-david]: https://github.com/onyedikachi-david

<!-- patches -->

[#3189]: https://github.com/calimero-network/core/pull/3189
[#3188]: https://github.com/calimero-network/core/pull/3188
[#3187]: https://github.com/calimero-network/core/pull/3187
[#3185]: https://github.com/calimero-network/core/pull/3185
[#3181]: https://github.com/calimero-network/core/pull/3181
[#3180]: https://github.com/calimero-network/core/pull/3180
[#3179]: https://github.com/calimero-network/core/pull/3179
[#3178]: https://github.com/calimero-network/core/pull/3178
[#3177]: https://github.com/calimero-network/core/pull/3177
[#3176]: https://github.com/calimero-network/core/pull/3176
[#3175]: https://github.com/calimero-network/core/pull/3175
[#3174]: https://github.com/calimero-network/core/pull/3174
[#3173]: https://github.com/calimero-network/core/pull/3173
[#3172]: https://github.com/calimero-network/core/pull/3172
[#3171]: https://github.com/calimero-network/core/pull/3171
[#3170]: https://github.com/calimero-network/core/pull/3170
[#3169]: https://github.com/calimero-network/core/pull/3169
[#3168]: https://github.com/calimero-network/core/pull/3168
[#3167]: https://github.com/calimero-network/core/pull/3167
[#3166]: https://github.com/calimero-network/core/pull/3166
[#3165]: https://github.com/calimero-network/core/pull/3165
[#3158]: https://github.com/calimero-network/core/pull/3158
[#3156]: https://github.com/calimero-network/core/pull/3156
[#3155]: https://github.com/calimero-network/core/pull/3155
[#3153]: https://github.com/calimero-network/core/pull/3153
[#3152]: https://github.com/calimero-network/core/pull/3152
[#3150]: https://github.com/calimero-network/core/pull/3150
[#3149]: https://github.com/calimero-network/core/pull/3149
[#3148]: https://github.com/calimero-network/core/pull/3148
[#3147]: https://github.com/calimero-network/core/pull/3147
[#3145]: https://github.com/calimero-network/core/pull/3145
[#3143]: https://github.com/calimero-network/core/pull/3143
[#3142]: https://github.com/calimero-network/core/pull/3142
[#3138]: https://github.com/calimero-network/core/pull/3138
[#3135]: https://github.com/calimero-network/core/pull/3135
[#3134]: https://github.com/calimero-network/core/pull/3134
[#3133]: https://github.com/calimero-network/core/pull/3133
[#3132]: https://github.com/calimero-network/core/pull/3132
[#3131]: https://github.com/calimero-network/core/pull/3131
[#3128]: https://github.com/calimero-network/core/pull/3128
[#3127]: https://github.com/calimero-network/core/pull/3127
[#3125]: https://github.com/calimero-network/core/pull/3125
[#3124]: https://github.com/calimero-network/core/pull/3124
[#3123]: https://github.com/calimero-network/core/pull/3123
[#3115]: https://github.com/calimero-network/core/pull/3115
[#3114]: https://github.com/calimero-network/core/pull/3114
[#3109]: https://github.com/calimero-network/core/pull/3109
[#3105]: https://github.com/calimero-network/core/pull/3105
[#3104]: https://github.com/calimero-network/core/pull/3104
[#3100]: https://github.com/calimero-network/core/pull/3100
[#3097]: https://github.com/calimero-network/core/pull/3097
[#3093]: https://github.com/calimero-network/core/pull/3093
[#3091]: https://github.com/calimero-network/core/pull/3091
[#3087]: https://github.com/calimero-network/core/pull/3087
[#3085]: https://github.com/calimero-network/core/pull/3085
[#3084]: https://github.com/calimero-network/core/pull/3084
[#3078]: https://github.com/calimero-network/core/pull/3078
[#3077]: https://github.com/calimero-network/core/pull/3077
[#3076]: https://github.com/calimero-network/core/pull/3076
[#3075]: https://github.com/calimero-network/core/pull/3075
[#3074]: https://github.com/calimero-network/core/pull/3074
[#3073]: https://github.com/calimero-network/core/pull/3073
[#3072]: https://github.com/calimero-network/core/pull/3072
[#3071]: https://github.com/calimero-network/core/pull/3071
[#3070]: https://github.com/calimero-network/core/pull/3070
[#2983]: https://github.com/calimero-network/core/pull/2983
[#3068]: https://github.com/calimero-network/core/pull/3068
[#3066]: https://github.com/calimero-network/core/pull/3066
[#3065]: https://github.com/calimero-network/core/pull/3065
[#3064]: https://github.com/calimero-network/core/pull/3064
[#3063]: https://github.com/calimero-network/core/pull/3063
[#3061]: https://github.com/calimero-network/core/pull/3061
[#3059]: https://github.com/calimero-network/core/pull/3059
[#3055]: https://github.com/calimero-network/core/pull/3055
[#3052]: https://github.com/calimero-network/core/pull/3052
[#3051]: https://github.com/calimero-network/core/pull/3051
[#3048]: https://github.com/calimero-network/core/pull/3048
[#3046]: https://github.com/calimero-network/core/pull/3046
[#3045]: https://github.com/calimero-network/core/pull/3045
[#3043]: https://github.com/calimero-network/core/pull/3043
[#3041]: https://github.com/calimero-network/core/pull/3041
[#3040]: https://github.com/calimero-network/core/pull/3040
[#3039]: https://github.com/calimero-network/core/pull/3039
[#3036]: https://github.com/calimero-network/core/pull/3036
[#3034]: https://github.com/calimero-network/core/pull/3034
[#3029]: https://github.com/calimero-network/core/pull/3029
[#3027]: https://github.com/calimero-network/core/pull/3027
[#3020]: https://github.com/calimero-network/core/pull/3020
[#3018]: https://github.com/calimero-network/core/pull/3018
[#3016]: https://github.com/calimero-network/core/pull/3016
[#3014]: https://github.com/calimero-network/core/pull/3014
[#3013]: https://github.com/calimero-network/core/pull/3013
[#3010]: https://github.com/calimero-network/core/pull/3010
[#3009]: https://github.com/calimero-network/core/pull/3009
[#3004]: https://github.com/calimero-network/core/pull/3004
[#2980]: https://github.com/calimero-network/core/pull/2980
[#2960]: https://github.com/calimero-network/core/pull/2960
[#2950]: https://github.com/calimero-network/core/pull/2950
[#2946]: https://github.com/calimero-network/core/pull/2946
[#2944]: https://github.com/calimero-network/core/pull/2944
[#2941]: https://github.com/calimero-network/core/pull/2941
[#2939]: https://github.com/calimero-network/core/pull/2939
[#2936]: https://github.com/calimero-network/core/pull/2936
[#2935]: https://github.com/calimero-network/core/pull/2935
[#2933]: https://github.com/calimero-network/core/pull/2933
[#2931]: https://github.com/calimero-network/core/pull/2931
[#2927]: https://github.com/calimero-network/core/pull/2927
[#2925]: https://github.com/calimero-network/core/pull/2925
[#2924]: https://github.com/calimero-network/core/pull/2924
[#2923]: https://github.com/calimero-network/core/pull/2923
[#2922]: https://github.com/calimero-network/core/pull/2922
[#2918]: https://github.com/calimero-network/core/pull/2918
[#2916]: https://github.com/calimero-network/core/pull/2916
[#2915]: https://github.com/calimero-network/core/pull/2915
[#2912]: https://github.com/calimero-network/core/pull/2912
[#2911]: https://github.com/calimero-network/core/pull/2911
[#2895]: https://github.com/calimero-network/core/pull/2895
[#2474]: https://github.com/calimero-network/core/issues/2474
[#2809]: https://github.com/calimero-network/core/pull/2809
[#2805]: https://github.com/calimero-network/core/pull/2805
[#2801]: https://github.com/calimero-network/core/pull/2801
[#2806]: https://github.com/calimero-network/core/pull/2806
[#2795]: https://github.com/calimero-network/core/pull/2795
[#2799]: https://github.com/calimero-network/core/pull/2799
[#2796]: https://github.com/calimero-network/core/pull/2796
[#2772]: https://github.com/calimero-network/core/pull/2772
[#2776]: https://github.com/calimero-network/core/pull/2776
[#2855]: https://github.com/calimero-network/core/pull/2855
[#2792]: https://github.com/calimero-network/core/pull/2792
[#2559]: https://github.com/calimero-network/core/pull/2559
[#2551]: https://github.com/calimero-network/core/pull/2551
[#1171]: https://github.com/calimero-network/core/pull/1171
[#1223]: https://github.com/calimero-network/core/pull/1223
[#1181]: https://github.com/calimero-network/core/pull/1181
[#1151]: https://github.com/calimero-network/core/pull/1151
[#1224]: https://github.com/calimero-network/core/pull/1224
[#1174]: https://github.com/calimero-network/core/pull/1174
[#1227]: https://github.com/calimero-network/core/pull/1227
[#1180]: https://github.com/calimero-network/core/pull/1180
[#1214]: https://github.com/calimero-network/core/pull/1214
[#1209]: https://github.com/calimero-network/core/pull/1209
[#1205]: https://github.com/calimero-network/core/pull/1205
[#1206]: https://github.com/calimero-network/core/pull/1206
[#1263]: https://github.com/calimero-network/core/pull/1263
[#1132]: https://github.com/calimero-network/core/pull/1132
[#1158]: https://github.com/calimero-network/core/pull/1158
[#1232]: https://github.com/calimero-network/core/pull/1232
[#1246]: https://github.com/calimero-network/core/pull/1246
[#1238]: https://github.com/calimero-network/core/pull/1238
[#1251]: https://github.com/calimero-network/core/pull/1251
[#1291]: https://github.com/calimero-network/core/pull/1291
[#1280]: https://github.com/calimero-network/core/pull/1280
[#1237]: https://github.com/calimero-network/core/pull/1237
[#1241]: https://github.com/calimero-network/core/pull/1241
[#1233]: https://github.com/calimero-network/core/pull/1233
[#1240]: https://github.com/calimero-network/core/pull/1240
[#1254]: https://github.com/calimero-network/core/pull/1254
[#1261]: https://github.com/calimero-network/core/pull/1261
[#1270]: https://github.com/calimero-network/core/pull/1270
[#1266]: https://github.com/calimero-network/core/pull/1266
[#1245]: https://github.com/calimero-network/core/pull/1245
[#1226]: https://github.com/calimero-network/core/pull/1226
[#1285]: https://github.com/calimero-network/core/pull/1285
[#1257]: https://github.com/calimero-network/core/pull/1257
[#1276]: https://github.com/calimero-network/core/pull/1276
[#1256]: https://github.com/calimero-network/core/pull/1256
[#1293]: https://github.com/calimero-network/core/pull/1293
[#1268]: https://github.com/calimero-network/core/pull/1268
[#1294]: https://github.com/calimero-network/core/pull/1294
[#1295]: https://github.com/calimero-network/core/pull/1295
[#1296]: https://github.com/calimero-network/core/pull/1296
[#1297]: https://github.com/calimero-network/core/pull/1297
[#1300]: https://github.com/calimero-network/core/pull/1300
[#1302]: https://github.com/calimero-network/core/pull/1302
[#1303]: https://github.com/calimero-network/core/pull/1303
[#1305]: https://github.com/calimero-network/core/pull/1305
[#1317]: https://github.com/calimero-network/core/pull/1317
[#1319]: https://github.com/calimero-network/core/pull/1319
[#1336]: https://github.com/calimero-network/core/pull/1336
[#1337]: https://github.com/calimero-network/core/pull/1337
[#1338]: https://github.com/calimero-network/core/pull/1338
[#1340]: https://github.com/calimero-network/core/pull/1340
[#1342]: https://github.com/calimero-network/core/pull/1342
[#1344]: https://github.com/calimero-network/core/pull/1344
[#1345]: https://github.com/calimero-network/core/pull/1345
[#1346]: https://github.com/calimero-network/core/pull/1346
[#1349]: https://github.com/calimero-network/core/pull/1349
[#1352]: https://github.com/calimero-network/core/pull/1352
[#1354]: https://github.com/calimero-network/core/pull/1354
[#1355]: https://github.com/calimero-network/core/pull/1355
[#1356]: https://github.com/calimero-network/core/pull/1356
[#1357]: https://github.com/calimero-network/core/pull/1357
[#1358]: https://github.com/calimero-network/core/pull/1358
[#1359]: https://github.com/calimero-network/core/pull/1359
[#1360]: https://github.com/calimero-network/core/pull/1360
[#1361]: https://github.com/calimero-network/core/pull/1361
[#1366]: https://github.com/calimero-network/core/pull/1366
[#1367]: https://github.com/calimero-network/core/pull/1367
[#1368]: https://github.com/calimero-network/core/pull/1368
[#1369]: https://github.com/calimero-network/core/pull/1369
[#1370]: https://github.com/calimero-network/core/pull/1370
[#1371]: https://github.com/calimero-network/core/pull/1371
[#1374]: https://github.com/calimero-network/core/pull/1374
[#1375]: https://github.com/calimero-network/core/pull/1375
[#1376]: https://github.com/calimero-network/core/pull/1376
[#1377]: https://github.com/calimero-network/core/pull/1377
[#1378]: https://github.com/calimero-network/core/pull/1378
[#1381]: https://github.com/calimero-network/core/pull/1381
[#1382]: https://github.com/calimero-network/core/pull/1382
[#1383]: https://github.com/calimero-network/core/pull/1383
[#1384]: https://github.com/calimero-network/core/pull/1384
[#1385]: https://github.com/calimero-network/core/pull/1385
[#1387]: https://github.com/calimero-network/core/pull/1387
[#1388]: https://github.com/calimero-network/core/pull/1388
[#1389]: https://github.com/calimero-network/core/pull/1389
[#1390]: https://github.com/calimero-network/core/pull/1390
[#1392]: https://github.com/calimero-network/core/pull/1392
[#1395]: https://github.com/calimero-network/core/pull/1395
[#1398]: https://github.com/calimero-network/core/pull/1398
[#1399]: https://github.com/calimero-network/core/pull/1399
[#1400]: https://github.com/calimero-network/core/pull/1400
[#1402]: https://github.com/calimero-network/core/pull/1402
[#1403]: https://github.com/calimero-network/core/pull/1403
[#1405]: https://github.com/calimero-network/core/pull/1405
[#1406]: https://github.com/calimero-network/core/pull/1406
[#1408]: https://github.com/calimero-network/core/pull/1408
[#1410]: https://github.com/calimero-network/core/pull/1410
[#1412]: https://github.com/calimero-network/core/pull/1412
[#1413]: https://github.com/calimero-network/core/pull/1413
[#1415]: https://github.com/calimero-network/core/pull/1415
[#1417]: https://github.com/calimero-network/core/pull/1417
[#1418]: https://github.com/calimero-network/core/pull/1418
[#1419]: https://github.com/calimero-network/core/pull/1419
[#1422]: https://github.com/calimero-network/core/pull/1422
[#1423]: https://github.com/calimero-network/core/pull/1423
[#1426]: https://github.com/calimero-network/core/pull/1426
[#1428]: https://github.com/calimero-network/core/pull/1428
[#1429]: https://github.com/calimero-network/core/pull/1429
[#1430]: https://github.com/calimero-network/core/pull/1430
[#1431]: https://github.com/calimero-network/core/pull/1431
[#1432]: https://github.com/calimero-network/core/pull/1432
[#1436]: https://github.com/calimero-network/core/pull/1436
[#1440]: https://github.com/calimero-network/core/pull/1440
[#1444]: https://github.com/calimero-network/core/pull/1444
[#1449]: https://github.com/calimero-network/core/pull/1449
[#1450]: https://github.com/calimero-network/core/pull/1450
[#1451]: https://github.com/calimero-network/core/pull/1451
[#1452]: https://github.com/calimero-network/core/pull/1452
[#1453]: https://github.com/calimero-network/core/pull/1453
[#1454]: https://github.com/calimero-network/core/pull/1454
[#1456]: https://github.com/calimero-network/core/pull/1456
[#1459]: https://github.com/calimero-network/core/pull/1459
[#1460]: https://github.com/calimero-network/core/pull/1460
[#1461]: https://github.com/calimero-network/core/pull/1461
[#1463]: https://github.com/calimero-network/core/pull/1463
[#1465]: https://github.com/calimero-network/core/pull/1465
[#1470]: https://github.com/calimero-network/core/pull/1470
[#1471]: https://github.com/calimero-network/core/pull/1471
[#1474]: https://github.com/calimero-network/core/pull/1474
[#1475]: https://github.com/calimero-network/core/pull/1475
[#1476]: https://github.com/calimero-network/core/pull/1476
[#1477]: https://github.com/calimero-network/core/pull/1477
[#1479]: https://github.com/calimero-network/core/pull/1479
[#1480]: https://github.com/calimero-network/core/pull/1480
[#1481]: https://github.com/calimero-network/core/pull/1481
[#1485]: https://github.com/calimero-network/core/pull/1485
[#1486]: https://github.com/calimero-network/core/pull/1486
[#1488]: https://github.com/calimero-network/core/pull/1488
[#1489]: https://github.com/calimero-network/core/pull/1489
[#1490]: https://github.com/calimero-network/core/pull/1490
[#1491]: https://github.com/calimero-network/core/pull/1491
[#1492]: https://github.com/calimero-network/core/pull/1492
[#1495]: https://github.com/calimero-network/core/pull/1495
[#1497]: https://github.com/calimero-network/core/pull/1497
[#1498]: https://github.com/calimero-network/core/pull/1498
[#1499]: https://github.com/calimero-network/core/pull/1499
[#1500]: https://github.com/calimero-network/core/pull/1500
[#1503]: https://github.com/calimero-network/core/pull/1503
[#1504]: https://github.com/calimero-network/core/pull/1504
[#1505]: https://github.com/calimero-network/core/pull/1505
[#1510]: https://github.com/calimero-network/core/pull/1510
[#1511]: https://github.com/calimero-network/core/pull/1511
[#1512]: https://github.com/calimero-network/core/pull/1512
[#1514]: https://github.com/calimero-network/core/pull/1514
[#1516]: https://github.com/calimero-network/core/pull/1516
[#1517]: https://github.com/calimero-network/core/pull/1517
[#1518]: https://github.com/calimero-network/core/pull/1518
[#1519]: https://github.com/calimero-network/core/pull/1519
[#1520]: https://github.com/calimero-network/core/pull/1520
[#1521]: https://github.com/calimero-network/core/pull/1521
[#1522]: https://github.com/calimero-network/core/pull/1522
[#3440]: https://github.com/calimero-network/core/pull/3440
[#3450]: https://github.com/calimero-network/core/pull/3450
[#3485]: https://github.com/calimero-network/core/issues/3485
[#3528]: https://github.com/calimero-network/core/pull/3528
[#3530]: https://github.com/calimero-network/core/pull/3530
[#3595]: https://github.com/calimero-network/core/pull/3595
[#3607]: https://github.com/calimero-network/core/pull/3607
[#3743]: https://github.com/calimero-network/core/pull/3743
[#3789]: https://github.com/calimero-network/core/pull/3789
[#3796]: https://github.com/calimero-network/core/pull/3796
[#3799]: https://github.com/calimero-network/core/pull/3799
[#3810]: https://github.com/calimero-network/core/issues/3810
[#3811]: https://github.com/calimero-network/core/pull/3811
