# calimero-node (the inside of `merod`)

The node: an attested TEE node that hosts the cluster's system
group and the app groups placed on it. Design: the "Calimero Confidential Cloud"
architecture spec. This crate is the library; the `merod` binary
(`crates/merod`: `init`, `run`, `config`, `local`, `inspect`, `kms`) turns its
configuration into a `Config` and calls `run`, and `meroctl` (`crates/meroctl`)
is its command line.

## Mental model

- **One Raft log per app** (`calimero-log`). The leader runs the WASM call
  and appends `{ call, outcome { result, diff, events } }`. Followers apply the diff;
  nothing is re-executed. `base_index` guards against applying a stale diff.
- **Two executors** (`types::Executor`; the transition the spec records as D53,
  overriding D1/D15 until v1 apps move): a v1 core module (SDK 0.12) runs on
  wasmer (`app::Executor`, today's `calimero-runtime` over a copy-on-write
  `Overlay`; everything below that is not under "Component executor"); a
  `calimero:app` component (the v2 SDK) runs on wasmtime through the host core
  (`node/component.rs`, `component.rs`). The executor is decided once, from the WASM
  the app's bundle names (a package's service or a raw upload,
  `wasmparser::Parser::is_component`), when the app is created
  (`AppRecord::executor`, absent = wasmer), and the app's `init` entry records it in
  the group's state (`AppState::executor`, in `Head`, so a snapshot carries it; a
  state from before has none: wasmer). Each executor's entries are a format of its
  own (`app::AppCmd`, `component::ComponentCmd`, distinct `op` names) and write a
  keyspace of its own (`rows::KV`/`IDEM` and `rows::CKV`/`CIDEM`/`CORDER`); a replica
  applies an entry with the executor its state names, and one of the other format
  changes nothing (tests `a_wasmer_entry_never_applies_to_a_component_app`,
  `wasmer_and_component_apps_share_a_cluster`). An upgrade that would move an app
  between executors is refused at the leader (`upgrade_local`), either way, until
  rebuild migrations (D38) exist (`upgrades_across_executors_are_refused_both_ways`).
  A data directory written before components opens and its apps run on wasmer
  (`old_datadir.rs`, from a directory `a09547b2b` wrote).
- **System group** (`system.rs`, group 1): fleet, app directory, bundles, accounts,
  passkeys, API keys, refresh tokens, organizations, teams, invites, used proofs.
  Commands carry their `actor`; `apply` checks authority, so the log decides.
- **Routing** (`node.rs`): system writes go to the system leader, calls and strong
  views to the app's leader, over **peer sealed channels** (`internal/*`) whose key
  is attested like any client's, plus the cluster secret (HMAC of K_c).
- **Peer identity and the inbound gate** (Phase 0 of
  `docs/specs/gate-system-group.md`): a node's channel to a peer is identified
  (Noise IK with its own transport key, `x-calimero-node: <id>`);
  `Node::identify_peer` attests the claimed node's registered address itself and
  takes the claim only if that quote names the proven key (cached by key; the
  quote's release with it). `api::peer` replaces the bare cluster-secret check
  and refuses an anonymous session. Routes: app data
  (`view`, `call`, `events`, `storage`, `blob/held`, `app-ready`, `presence`)
  need a release approved for the app (`caller_may_hold`); `blob/get` code to any
  node, other blobs only for an approved hosted app that marks them; `blob/delete`
  refused while a hosted app marks the blob; `raft` needs `rpc.sender()` = the
  caller and, for an app group, an approved release and membership (replicas or
  the group's voters/learners): openraft itself never checks (F4). Outbound,
  blobs (`spread_blob`) and presence go only to peers `may_send` allows.
  No interop with nodes before this (a hard cutover: clusters are recreated).
  Regression (`tests/approval_bypasses.rs`):
  `a_node_on_an_unapproved_release_reaches_no_app_through_internal_routes`.
- **Decisions carry proofs** (`proof.rs`, `system::authorize`, run before every command):
  consent is logged as `{ subject, method, args, nonce, assertion }`
  (`api::consent` keeps the nonce in `Pending::Confirm`); `apply` rebuilds
  `confirm_challenge`, verifies it under a passkey of the actor
  (`calimero_identity::verify_consent`), checks the statement says what the
  command does, and spends it (`used_proofs`). Operator decisions carry an
  Ed25519 signature under `SystemState::operator_key` (`--operator-key`,
  recorded at genesis); ops routes answer `428
  OPERATOR_SIGNATURE_REQUIRED` with the bytes to sign (`api::operator_proof`).
  An org run is consented once (`StartUpgradeRun { consent }`: `__upgrade_org`,
  its retry, or a publisher's `__upgrade_published` over every instance of its
  package); its per-app `UpgradeApp { run }` is judged by the run, found in
  whichever organization keeps it. `BindPublisher` carries a passkey consent
  over the key's current binding generation; `SetReleaseDeadline` the
  operator's signature (`cluster / __release_deadline / { version, after }`;
  `after: null` lifts it). The consent page (`consent_page::act`) logs the same
  consent as the API, its assertion made on the node's own origin (`apply`
  checks the signature and the statement, not the origin). Tests run
  commands' own authority rules through `run_authorized` (`run_unproven` with
  the new-work refusal). Test clusters start with the harness's
  operator key (`ops_signed` signs like mdma). Regressions: `forged_decisions_are_refused_where_they_apply_on_every_node`,
  `operator_decisions_need_the_operator_signature`, `system::tests`.
- **API** (`api.rs`), all inside the sealed channel except `/attestation`, `/health` and the consent page:
  `/auth/*` (passkeys, refresh, API keys), `/bundles`, `/apps`, `/apps/{app}/{intents,views,events,upgrade,access}`,
  `/orgs/*`, `/invites/{token}/redeem`, `/consent/*`, `/internal/*` (peers), `/ops/*` (mdma: counts, placement and version reports by id only, no read path into apps).
- **Reserved methods** (`node::reserved_method`, `__*`): the node's own commands
  (`HOST_METHODS`: `__upgrade`, `__blob`, `__blob_forget`, `__file_forget`) run only for a
  `CallRequest::host` request, which only the API builds after judging who may;
  `/intents` and `/views` refuse a `__` method (`400 RESERVED_METHOD`), and an
  xcall or follow-up naming one is refused at the leader. Regression:
  `the_nodes_own_commands_cannot_be_called_as_methods`.
- **Ceremonies** (`api::Pending`): a challenge lives on the node that began it, its id `{node}.{random}`; finishing elsewhere takes it from that node (`internal/challenge/take`), so it is used once. Regression: `a_sign_in_begun_on_one_node_finishes_on_another`.
- **Confirm methods**: `intents` answers `412 CONFIRM_REQUIRED` with a challenge over
  the exact call; the retry carries a passkey assertion, which is logged as consent.
- **Bundles are blobs, not log entries**: `POST /bundles` stores the WASM in every
  node's sealed blob store (`internal/blob`, majority required), then the system log
  records only `{hash, size}`. A node missing a blob fetches it from a peer
  (`internal/blob/get`) on first use. Log entries are capped at
  `calimero_log::MAX_ENTRY_BYTES` (1 MiB): openraft gives each append one heartbeat.
  A call whose arguments alone pass the cap is refused where it arrives
  (`413 TOO_LARGE`), never forwarded to the leader first (regression:
  `a_write_too_large_for_the_log_is_refused_at_once_on_every_node`).
- **Packages** (`packages.rs`, `system::PackageRecord`): signed `.mpk` releases.
  `POST /packages/install { package, version }` fetches from the
  operator's registry (`--registry-url`, `calimero-app-downloader`'s `HttpRegistry`);
  `POST /packages` takes an uploaded file, only with `Config::allow_package_uploads`
  (`--dev`; else `403 UPLOADS_DISABLED`: a production release comes from the
  registry, which any node can fetch it from again). Before `InstallPackage` is logged the
  node checks the publisher signature and every artifact digest
  (`calimero_bundle::VerifiedBundle`), `minRuntimeVersion` (build.rs's
  `CALIMERO_RELEASE_VERSION`), the package's pinned signer (first release wins,
  `apply` re-checks) and that each service's WASM compiles. The log keeps the
  `.mpk`'s hash; a node that runs a release it does not hold fetches it itself
  (`Node::release_wasm`: the registry for a registry release, never a peer; a peer
  for an upload), refuses bytes whose hash differs, and verifies them again.
  `POST /apps { package, version?, service? }` creates from a release (installing
  it from the registry first); upgrade takes `{ version }`; `DELETE /apps/{app}`;
  `DELETE /packages/{p}[/{v}]` is refused while an app runs the release (a WASM
  another release ships stays). Raw `POST /bundles` is development only
  (`allow_unsigned_bundles`, on with `--dev`). **Who sees a release**
  (`SystemState::sees_release`): its installers (`installed_by`, and
  `PackageVersion::installers`, added when someone installs the same release
  again), everyone for a release the operator installed (the catalog), accounts
  sharing an organization with an installing account, accounts that reach an
  app running it, a session bound to the package's frontend (all its releases),
  and the operator for its own installs only. For
  anyone else it is not installed: `GET /packages[/{p}]` and `GET /ops/packages`
  leave it out (`404` by name), `resolve_release` treats it as missing (a
  registry install makes it theirs; an upload answers as a missing release), a
  bundle hash it ships is an unknown bundle (`sees_bundle`), outbound approval
  answers `404`, and upgrade runs skip it (`api::upgrade_sees`; the operator's:
  its own or the app owner's). Frontends stay public by name. Regressions:
  `a_private_package_is_listed_only_to_accounts_that_may_use_it`,
  `a_release_from_before_installers_reads_and_records_who_installs_it_again`.
- **Governance** (`system::Governance`, `Authority`, shared with clients in
  `calimero_client::attest`): three decisions, each held by exactly one
  party chosen at deployment, `owner` (default) or `operator`:
  `--release-approvals` (which mero-tee releases may hold the data),
  `--app-upgrades`, `--outbound-approvals`. The first leader writes
  `SystemCmd::Genesis` before any account exists (`CreateAccount` waits for it);
  a second genesis is refused, so a restart with other flags changes nothing.
  Every receipt-key quote binds it (`receipt_key_binding(key, governance)`,
  `AttestationDoc::governance`), so `verify_app_cluster` proves it. Operator
  side: `POST /ops/releases` (`operator_releases`, which `approved_releases`
  then answers), `POST /ops/apps/{app}/upgrade` (an installed release;
  `UpgradeApp { by_operator }`), `POST /ops/outbound` (`operator_outbound`,
  `ReleaseScope::Cluster`). The operator installs the releases it upgrades
  to: `POST /ops/packages` (an `.mpk`), `/ops/packages/install` (the
  registry), `GET /ops/packages`, `DELETE /ops/packages/{p}[/{v}]` (only what
  it installed), recorded as `system::OPERATOR`, which no account can be and
  which no account quota counts. Owner side: the passkey routes below. Whichever
  party does not hold a decision gets 403 (API) and a refusal (`apply`).
- **The approval model** (where governance says `owner`; the same on every
  cluster, shared or dedicated). Decision × who holds it × who acts × where:
  - *Node images* (`SystemState::approved_for_bundle`): the app's **publisher**,
    the org its package's signing key is bound to (`BindPublisher`,
    `POST /orgs/{org}/publishers { signerId, publicKey, signature }` by an
    `APPROVE_RELEASES` holder with passkey consent; the key's Ed25519 signature
    over `system::publisher_binding_message` (cluster name, org, key, the key's
    generation from `GET /publishers/{signer}`), verified at apply; each binding
    bumps the generation, so a signature never replays; rebinding needs a new
    signature from the pinned key; `SystemState::publishers`). `PackageRecord::owner`
    is only the first installer and is never the publisher. The publisher's
    `Org::approved_releases` cover every instance of every package the key
    signs, customers' included. Unbound key or raw bundle: the app's own org's
    set, or its owner account's. Acted on by members holding
    `caps::APPROVE_RELEASES` (bit 10; owner and admins hold it, a member when
    granted org-wide; `SystemState::approves_releases`, 403 at the API and
    refused at apply), or by the account for its own set
    (`POST /orgs/{org}/releases`, `/auth/releases`, logged as consent). Enforced
    by the replicas that hold the data: `Node::may_send` refuses Raft
    entries/snapshots to a peer whose attested release
    (`HttpAttestor::seen_release`) is not approved, `Node::place` skips such
    nodes, `/ops/apps/{app}/replicas` refuses them.
  - *App upgrades*, per app: the package's signed manifest says
    `upgrades: owner | publisher` (`calimero_bundle::UpgradeAuthority`, kept on
    `PackageVersion::upgrades`; absent = `owner`). `owner`: the app's owner, or, for an org app, whoever holds
    `MANAGE_APPLICATION` on its team or org-wide (`SystemState::manages_apps`).
    `publisher`: the publisher's org-wide `MANAGE_APPLICATION` holders, for
    every instance (`POST /orgs/{org}/packages/{package}/upgrade`), not the
    instance's owner, and nobody (`409`) while the key is unbound.
    Only loosens toward owners: `PackageRecord::upgrades_in_order` at install,
    `SystemState::upgrade_keeps_authority` at upgrade (API and apply). Each with
    their own passkey assertion (`consent` on `UpgradeApp`). `run_upgrade` judges
    who and where before anything runs on the app's group.
  - *Release deadlines* (`SetReleaseDeadline`, `SystemState::release_deadlines`):
    the operator's, any governance, `POST|DELETE /ops/releases/{version}/deadline`
    (signed with the operator key, `__release_deadline`; `after: null` lifts),
    public at `GET /releases`. Past one, a party whose newest approved release is
    older (judged on the set that governs the app) creates, installs and upgrades
    nothing (`SystemState::stalled_for_bundle`, `install_stalled`; `409
    RELEASE_DEADLINE`, `node::stalled`), placement skips retired nodes
    (`SystemState::retired`); running apps go on. Judged at the command's own
    time (`createdAt`, `at`, `installedAt`), never a node's clock; `run_upgrade`
    stamps `at` before the migration so apply agrees. No mixed-version
    support: every node runs the same release.
- **Placement** (`Node::place`, `placement::choose`): among the approved nodes,
  a new app's `--replicas` span as many zones (`NodeRecord::zone`, from
  `POST /ops/nodes`) as they are in, least loaded (then lower id) first within
  that; zoneless nodes share one unknown zone, so a cluster without zones is
  placed by load as before. Regressions: `a_new_app_is_placed_across_zones`,
  `placement_across_zones_keeps_to_approved_releases`, `placement::tests`.
- **Outbound HTTPS** (`calimero-outbound`): `Config::outbound` from
  `--settings` (`[outbound]`). `Node::outbound_for` hands a client only to
  `call_local`'s runs (calls, follow-ups, timers; not a read-only member's), for
  a bundle that is a signed release declaring prefixes; identity = (package,
  pinned signer), approval = `Org/Account::outbound_approvals[signer]`, set by
  the account, or for an org whoever holds `MANAGE_APPLICATION` org-wide, with
  `SetOutboundApproval` (`POST /orgs/{org}/outbound`,
  `/auth/outbound`, passkey consent; `merge_approval` at apply). The request is
  made inside the propose lock, on its own thread (`run_blocking`); a leader-race
  re-run sends it again.
- **Files** (`files.rs`, `files/seal.rs`, `files/wrap.rs`): a file is 64 KiB AES-256-GCM STREAM chunks
  (`FileSealer`, `FileOpener`) under a random data key.
  The nonce is 4 zero bytes then the big-endian chunk index; the AAD is `calimero.file.v1` then a last-chunk flag byte.
  `FileOpener::finish_expecting` checks the plaintext size and SHA-256 against the mark, as GCM does not commit to its key.
  `wrap_key` wraps the data key as salt || nonce || AES-256-GCM under a subkey of `backup::file_key` (per app, from K_c, not the TEE key), with the app id as AAD.
  `wrap_for_job` re-wraps a secret to a job node: ephemeral X25519, then HKDF over both public keys, the kind (file key or noise seed), the length-prefixed app and run ids, attempt and index.
  The job node opens it through `SealedTransport::derive`, which never returns the raw DH output and labels its HKDF (`calimero_sealed::derive_shared`, shared with the wrap side).
  `noise_seed` and `run_token` derive a run's seed and capability from the file key.
- **Receipts**: every `/intents` and `/views` answer carries a `Receipt` signed by the
  answering node's receipt key (Ed25519, made on the node's first start and kept
  in `<data_dir>/signing-key` wrapped under K_r, `signing_key.rs`: a restart
  keeps it, only the release whose mero-kms gives that K_r opens it), over
  `(app, index, SHA-256(JCS(result)), node)`. The key is attested on its own
  (`GET /attestation?purpose=receipts`, report data `nonce || receipt_key_binding`),
  and `GET /apps/{app}/attestation` gathers every replica's
  (`Client::verify_app_cluster`).
- **KMS evidence**: `GET /attestation?purpose=kms` adds `kms` (`config.kms_keys`: the
  KMS quote merod accepted for its storage key, and the boot script's record for the
  data disk's, `--kms-disk-evidence`) and binds it (`kms_report_data`). Without a
  record, or without `purpose=kms`, the answer is the plain transport form.
- **Release attestations** (`attest.rs`, `release_attestation`, `serve_release`): a
  quote over a manager's nonce and `ReleaseClaim`, for `merod attest` before the node
  has any key. Not a route of a running node: `serve_release` is its own tiny server
  (that one route, `MAX_CONCURRENT_RELEASE_QUOTES`), run only by `merod attest --serve`.
- **The host's key space** (`app::RESERVED`, `\0calimero.*`): private spaces, writer
  sets, blob marks. App code's overlay answers nothing there and a run that tries is
  refused; the host writes with `Overlay::set_host` and reads with
  `Storage::host_get`. Rotations are judged by `rotation_refusal`.
- **Access is judged fresh**: every request carrying a credential, every gated request and every client call's leader
  read the system state through `sys_fresh` (a read index), so a removal committed
  elsewhere holds before the next request.
- **Joining nodes**: a node started without `--bootstrap` waits; once
  `/ops/nodes` registers it, the system leader adds it to the system group as a
  learner (`Node::admit_registered_nodes`, from `reconcile`), it catches up and
  learns the directory, and replica moves (`/ops/apps/{app}/replicas`) can put
  apps on it. While the system group has fewer than `membership::SYSTEM_VOTERS`
  (3) voters, the system leader promotes registered learners at most
  `membership::PROMOTE_MAX_LAG` (16) entries behind its log so that it has
  exactly 3, in one membership change (`Node::promote_learners`, from
  `reconcile`; the choice is `membership::grow_voters`: zones no voter is in
  first, then no zone, then the rest). Never 2 (no failure tolerated, every
  write waits on both): a cluster started on one node waits for two learners,
  then goes from 1 to 3; never past 3 on its own (5 would be a later flag).
  Regressions: `a_cluster_started_on_one_node_grows_and_survives_losing_it`,
  `membership::tests`.
- **Removing nodes**: `DELETE /ops/nodes/{id}` (system leader; `421 NOT_LEADER` elsewhere) writes `RemoveNode` (refused while an app keeps a replica there) and changes the system group's voters, promoting a registered learner while fewer than 3 are left; `SystemState::removed_nodes` keeps `reconcile` from registering it again from `--peers`. Regression: `a_node_is_removed_once_no_app_keeps_a_replica_on_it`.
- **Job nodes** (`NodeRole::Job`, `job_node.rs`, `node/job.rs`): `POST /ops/nodes { id, addr, zone?, role? }` (`"replica"` default, `"job"`; any other role is refused) registers a node that only runs compute. It is in the directory but never a Raft member (not even a learner: a member keeps its log sealed under K_c, which a job node never holds), never a replica, blob or bundle target, majority member, presence peer, CSP origin or cluster-key recipient. Every site that picks nodes for the cluster's own work reads `SystemState::replica_nodes()` / `replica_node(id)`, never `nodes`; `/ops/apps/{app}/replicas` naming a job node answers `400 JOB_NODE`, `internal/cluster-key` for one is refused, `/ops/placement` lists it with `role` and app counts stay replica-only. `DELETE /ops/nodes/{id}` removes one without touching system voters. A node's role is fixed (`RegisterNode` refuses another role for an id; remove it first) and `/ops/nodes` refuses `role: job` for a system voter, learner or `--peers` id (`NODE_HOLDS_CLUSTER`); removal changes system membership only for a node that is in it. Nodes are reached by id: Raft's `PeerTransport::send` and `internal/cluster-key` take the address `Node::replica_addr(id)` holds (the directory's, else `--peers`; none for a job node), and address-only callers (`Node::peer`, so `peer_post` and streams) resolve it by a normalised comparison (`system::normalize_addr`) and refuse anything that is not a replica's, so the cluster secret never leaves for a job node. `RegisterNode` refuses an address another id holds (`ADDRESS_TAKEN` at the API). A ceremony holder is resolved through `replica_node`. Registering a job node (`POST /ops/nodes`, `role: job`) answers `409 CLUSTER_NOT_UPGRADED` unless every replica node, voter and learner runs at least this node's release, so no job record exists while an older node could misread it (upgrade the voters first). Upgrade the voters before anything else: the `RegisterNode` refusals (role fixed, address taken) bind only nodes that run this release. A node registered as a replica is admitted as a learner only after it attests its transport key as a cluster node (`attests_as_replica`): a job node registered as a replica never is, and `--peers` must list only replica nodes. `merod run --job-node` calls `run_job(JobConfig)`: no store, K_c, KMS, Raft or system group; it serves `/health`, `/attestation` (only `purpose=jobs`, answering `jobKey` bound with the transport key, release, cluster and node id in one quote; every other purpose or none is 404, so no channel that attests a transport key opens to it; a cluster node answers `purpose=jobs` with 404) and `GET /job/load` (`{ running, capacity }`, `RUNS_PER_JOB_NODE` = 2). Its Ed25519 job key is made in memory at start. Leader side: `Node::attest_job(id, app)` fetches `purpose=jobs` over a fresh nonce, verifies it with `verify_job_key` under the peer policy (`--peer-profiles`, `--peer-min-release`), keeps the result `JOB_ATTEST_TTL` (60 s) for as long as the directory's `node_epoch` for the id is unchanged (any registration or removal) and until a channel to it fails (`forget_job`); it fails outright when the directory could not be brought up to date. `attest_job` also refuses a release the app's org or account has not approved (`release_may_hold`, as `may_send` does), and `/ops/placement` reads a job node's release through the same cached attestation; `Node::job_load(peer)` asks over a sealed channel pinned to the attested transport key. Regressions: `tests/job_nodes.rs`, `placement::tests::a_job_node_is_never_placed`.
- **Job runs** (`job_node/{admission,bundles,runs,exec}.rs`, `job_wire.rs`; tests `job_runs.rs`): a job node runs one attempt of a compute run per assignment, all inside the sealed channel. **Who may give it work**: only the cluster whose identity key it was started with (`merod run --job-node --cluster-identity HEX`, from `GET /ops/cluster` / `meroctl ops cluster`). The identity is an Ed25519 key derived from K_c (`backup::cluster_identity_key`, seed `HMAC-SHA256(K_c, "calimero.cluster-identity.v1")`), so a valid signature shows a node of this cluster signed; the job node fetches and attests nothing about the sender, and has no `--peers`; a key that is not a curve point or is of small order is refused at start (`check_cluster_identity`), signatures are checked strictly, and the identity is bound into the job node's `purpose=jobs` quote (`AttestationDoc::cluster_identity`, `job_key_binding`) so the leader's `attest_job` refuses a job node that serves another cluster. `PUT /job/bundles/{hash}` takes a component only with `x-bundle-signature`, the identity's signature over `"calimero.compute.bundle.v1\0" || hash` (`403 BAD_SIGNATURE`), then checks the hash, prepares it once (`MAX_JOB_BUNDLES` kept, least recently used first). `POST /job/runs` (at most `MAX_ASSIGNMENT_BYTES`) takes an `Assignment` (`job_wire`: app, run, attempt, method, args, bundle, caller, owner, `startTsMs`, budget, inputs `{id = hex SHA-256 of the ciphertext, size, sha256, wrapped}`, wrapped seed, hex token, `signature`) signed by the identity over a length-prefixed binary message of every field (`assignment_message`, big-endian integers; `Node::sign_assignment` is the cluster side). Refusals, in order: shape (`400 BAD_ASSIGNMENT`/`BAD_BUDGET`, duplicate input ids, `413 TOO_MANY_INPUTS`/`INPUT_TOO_LARGE`/`INPUTS_TOO_LARGE`), `403 BAD_SIGNATURE`, then an assignment already held answers its state (`409 RUN_CONFLICT` if it differs), `503 BUSY` at `RUNS_PER_JOB_NODE` runs waiting or running or when the finished outputs held on disk plus the most each active run and the new one may still write exceed `MAX_FINISHED_BYTES` (nothing is evicted, the leader collects or deletes), `409 NEED_BUNDLE`, `400 NOT_COMPUTE` (the bundle's ABI must name a compute method), `400 BAD_WRAP` (every wrapped key opens at admission through `unwrap_for_job`, so a key made for another node, run, attempt or index never gets in). Runs are keyed by `(app, run, attempt)` (run ids are per app) and every other route is `/job/runs/{app}/{run}/{attempt}/...` with the app id URL-encoded; they need the run token in `x-run-token` (constant-time compare); a wrong token answers exactly as an unknown run, `404 UNKNOWN_RUN`, which the leader treats as lost. `PUT .../inputs/{i}` stores the ciphertext only if its length is `sealed_len(size)` and its SHA-256 is the input's id (`400 BAD_INPUT_SIZE`/`BAD_INPUT_ID`), and only if it opens under the run's key to a file of the declared size and plaintext SHA-256 (`exec::input_matches`, decided once, from memory); an input that does not match ends the run on the spot with a signed `integrity` report, since the leader sent it; the run starts on its own OS thread when every input is in (at once for none). Sealed request bodies are decrypted and buffered before any route check, bounded by the sealed in-flight cap; that is accepted. `GET .../{attempt}` answers `{ state: awaiting|running|done|lost, progress?, report?, signature? }`: `done` carries the signed report, a failed run included; `lost` is a run this node could not serve (`Unavailable` trap, thread panic, node file I/O, an input that fails to open during the run, which is the node's own disk), never signed, which the leader restarts elsewhere with the next attempt. `GET .../outputs/{i}` and `.../key` give an output's ciphertext and data key (hex) once done; outputs are listed in the order the run finished them. `DELETE` sets the cancel flag, waits `CANCEL_WAIT` for the run to stop, answers its last status and drops it with its scratch files (a run that did not stop stays and answers `running`); runs not deleted are swept `JOB_RESULT_TTL` after they end, runs still waiting for inputs end lost and are swept after `AWAITING_INPUTS_TTL`; the scratch directory is emptied at start, so a restarted node knows no runs. The run is a `calimero:app` action over the files of the assignment only: inputs are streamed from the scratch ciphertext through `FileOpener` and checked with `finish_expecting` at the end, and a failure then is a read error, so the run is lost, never signed as `integrity`. Outputs are sealed to disk under a fresh key each (`NewFile.ciphertext` stays empty); engine `WasmtimeEngine::for_compute`, `invoke_cancellable`. A trap maps to the reason structurally (`exec::classify`): fuel, memory, time (`Timeout`), cancelled, trap (fault, stack, any other denied call), app-error. The `Report` (`job_wire`) holds app, run, attempt, status, reason, the result bytes, outputs `{id = SHA-256 of the ciphertext, size, sha256}` and usage as integers (`cpuMicros` thread CPU, `gibMicros` peak memory times wall, `fuel`); the job key signs `"calimero.compute.report.v1\0"`, the SHA-256 of the assignment message it ran (so a report stands for that assignment only) and the length-prefixed fields, output ids as their 32 decoded bytes (`report_message`); one encoding only: ids and hashes are lowercase 64 hex, a done report has no reason, a failed one has a reason and neither result nor outputs, and `verify_report(job_key, assignment_digest, report, signature)` is the one check the leader uses. `Assignment`'s `Debug` hides the token and the wrapped keys; the seed and keys live in `Zeroizing`, except the copy of the seed the host core keeps in its `Invocation` for the run's life. A job node feeds no metrics series: a compute run leaves every `calimero_app_*` series of the cluster unchanged until the leader counts it. The leader's reads from a job node (`attest_job`, `job_load`) stop at `MAX_JOB_RESPONSE_BYTES`.
- **Transfers**: `POST /apps/{app}/transfer { to }` (owner, passkey consent; an org app only to a member). Regression: `an_owner_transfers_an_app_with_their_passkey`.
- **Stopping** (`Running::stop`): cancel the tasks, drain the server (2 s),
  stop the groups, then wait (up to `STOP_WAIT`, 10 s) until the store is
  closed (`Engine::store_watch`): a task still running holds the RocksDB lock
  that `inspect` or a restart in the same process needs. Regression:
  `a_stopped_nodes_store_can_be_inspected` (it failed about 1 in 32 runs under
  load before).
- **Ops credentials and metrics**: `--ops-token-sha256` gives the node only the
  token's hash (machine metadata then holds nothing usable); `GET /ops/metrics`
  is Prometheus text (directory counts, this node's groups, streams, system
  membership, build release; per-group Raft term, lags and quorum age; store
  sizes; and, from `metrics::track` on the router, requests by route template
  and status, error codes, latency per route group and body bytes), never
  names or state: labels are templates, groups, codes and numbers, and ids only
  on the usage series below
  (regression: `metrics_count_requests_errors_and_raft_without_naming_anyone`). `--metrics-listen` (`Config::metrics_listen`) serves the same text
  at plain-HTTP `GET /metrics` for a scraper, nothing else (`api::plain_metrics`).
- **Usage metering** (`usage.rs`, `Node::usage`; billed from `/metrics`):
  `calimero_app_{calls_total{kind},wasm_cpu_seconds_total,wasm_memory_byte_seconds_total,outbound_requests_total,state_bytes,blob_bytes}`,
  labelled `app` and `org` (the payer: org id, or `account:{owner}` for a
  personal app; `usage::payer`). Every WASM run goes through `Node::metered`
  (`off_workers` plus `usage::measure`: thread CPU time, `CLOCK_THREAD_CPUTIME_ID`,
  around the run on its one thread; wall time off Unix), so CPU is counted on
  the node that ran the WASM: the leader for calls, `init`, migrations,
  follow-ups and timers (followers apply diffs and count nothing), the
  answering replica for a view. A call is counted once, after its proposal, by
  the run whose answer stood (not a conflict, not a component timeout); a
  discarded run adds only CPU and memory. Memory = the module's declared
  linear memory (`usage::declared_memory`, learned in `module_for` and
  `prepare_now`) × wall time: a floor, as the runtimes do not report an
  instance's peak. Outbound counts requests handed to the transport
  (`OutboundService::for_run_counted`). `sample_usage` fills the state and blob
  gauges every `usage::SAMPLE_EVERY` (5 min, `lib.rs`); `metrics_response`
  drops the series of apps no longer hosted here. In memory: counters restart
  at zero with the process. Regression:
  `app_usage_is_counted_on_the_node_that_ran_the_wasm`; docs: Observability,
  "Usage metering".
- **Ops replica moves** run on the app group's leader; any other replica answers
  `421 NOT_LEADER` with `details.leaderUrl` before writing anything. A move to
  the set the app already has writes nothing (`unchanged: true`); a second move
  of an app while one runs on this leader is refused `409 MOVE_IN_PROGRESS`
  (`Node::begin_move`, a guard the move's own task holds, so it runs to the end
  even if its caller disconnects). Before the new nodes become learners, the
  leader hands them the app's bundle and the blobs its state marks
  (`Node::hand_over_bytes`, over `internal/blob`): a bundle or blob stored while
  the cluster was one node is held by that node alone ("a majority" of one).
  Regressions: `replica_moves_are_idempotent_and_one_at_a_time`,
  `a_cluster_started_on_one_node_grows_and_survives_losing_it` (node 1 stops
  after the move; the app still runs and its blob downloads).
- **Placement read** (`GET /ops/placement`, `meroctl ops placement`, for mdma's
  rebalancer): each app's id, group and replicas, each node's id, zone,
  attested release (`Node::release_of_node`, at most `PLACEMENT_RELEASE_WAIT`)
  and app count, after `sys_fresh` (`fresh` in the answer). Ids and counts only.
- **Version and approval reports** (`report.rs`, read-only): `GET /ops/apps`,
  `GET /ops/orgs` (operator), `GET /orgs/{org}/apps` (any member; `404` else) and
  `GET /auth/apps` (the account's own apps), paged (`?after&limit`,
  `report::MAX_REPORT_PAGE`). Per app: scope, bundle digest, package release,
  `upgradesHeldBy`, `lastUpgrade` (`AppRecord::last_upgrade`, with
  `report::actor_kind`: `owner`/`member`/`operator`, judged against the app's and
  org's owners as they stand), approved releases, each replica's release and
  whether approved; for accounts also name, the upgrader's account, newer
  visible releases (`availableVersions`) and `clusterReleases`. The operator
  sees a package name only for releases it installed (`Reader::Operator`,
  `sees_release`), never an account, a name or an upgrade's error text.
  Node releases come from `Node::release_of_node` (attested once, then kept;
  at most `PLACEMENT_RELEASE_WAIT` each), as for placement. Regressions:
  `app_versions_and_release_approvals_are_reported_without_names`,
  `an_operator_upgrade_is_reported_as_the_operators`.
- **Forwards are bounded**: a call or read forwarded to an app's leader waits at most
  `FORWARD_TIMEOUT` (5 s, `peer_forward`), then retries at whoever leads now; a frozen
  leader would otherwise hold a write for the 30 s HTTP timeout. Calls without a
  client key get a node key (`node:…`, kept 10 min) first, so a retry replays a call
  that did commit. System-log forwards keep the long timeout: some system commands
  are not idempotent yet. Regression: merobox `workflow-examples/cloud-v2/frozen-leader.yml`.
- **Leader races**: `Group::propose` takes no leadership round before a call's
  append, so a deposed leader may run a call on stale state; its entry never
  commits, or commits as a no-op (`base_index` ≠ the app's last entry,
  `AppState::last_index`: `conflict`, no idempotency record), and `Node::call`
  re-runs it instead of failing. The base is the app's last entry, never the
  log's applied index, which counts blank and membership entries too: with it
  the first call after every election conflicted once (regression:
  `the_first_call_after_a_leader_change_commits_without_a_rerun`;
  `calimero_build_conflicts_total` in `/ops/metrics` counts conflicts). Replies without an append (idempotent replays) are confirmed first.
  The module is compiled from the bundle read before the lock, so the build
  also answers a conflict when the state names another bundle by then (an
  upgrade applied in between; `AppMachine::stale_module`, also checked by
  `upgrade_local`). Regression:
  `a_call_built_with_a_module_an_upgrade_replaced_never_commits`.
- **Runs leave the async workers** (`node::off_workers`): a WASM run (call,
  follow-up, timer, init, migration, strong view) happens inside the group's
  proposal lock, on the machine's read lock, from borrowed state, so it cannot
  move to `spawn_blocking`; it goes through `block_in_place`, which hands the
  worker's other tasks (Raft heartbeats, the system group, HTTP) to another
  worker meanwhile. Regression: `a_long_run_does_not_hold_an_async_worker`.
- **Upgrades plan their migration from the ABIs** (`services::plan_upgrade`):
  without a named `migrate`, an upgrade one state version up runs
  the target's `#[app::migrate]` edge; a downgrade, a jump or a missing edge is
  refused, and so is a target whose ABI strips authorship from a field
  (`calimero_wasm_abi::downgrade::identity_downgrades`, the L1 gate; regression:
  `an_upgrade_that_strips_authorship_is_refused`). Bundles without an ABI upgrade code-only. In a migration
  (`Executor::migrate`) the export returns the new root state, the app's
  `__calimero_migration_check` must pass, and the host writes the root entry
  (`write_root_state`). The directory records the new bundle only once the
  upgrade succeeded.
- **Upgrade runs** (cascade, status, retry): `POST
  /orgs/{org}/upgrade { package, version, team?, cascade? }` (the org owner, one
  passkey assertion for the run) or `POST /ops/orgs/{org}/upgrade` (where the
  operator holds app upgrades) records a `types::UpgradeRun` in the org
  (`StartUpgradeRun`, newest `UPGRADE_RUNS_KEPT`), then upgrades its apps one by
  one in a background task; each `UpgradeApp`/`UpgradeFailed` notes the app on
  the run and on `AppRecord::last_upgrade`. `GET /orgs/{org}/upgrades/{run}`
  follows it; `.../retry` runs the failed apps again as a new run. A release
  missing from the cluster is installed from the registry first (owner runs).
- **Aliases**: `Account::aliases`, `PUT/GET/DELETE /auth/aliases/{alias}`
  and `GET /auth/aliases`; an alias names only an app its account reaches.
- **Metadata and org lifecycle** (org/member/app metadata, device
  labels, owner-delete): `Org::metadata` (admins or `MANAGE_METADATA`),
  `Org::member_metadata` (the member or an admin), `AppRecord::metadata` (owner
  or org admin), each at most `MAX_METADATA_BYTES` (16 KiB, team metadata too);
  `Account::passkey_labels` (`POST /auth/passkeys/{cred} { label }`).
  `DELETE /orgs/{org}` is the owner's, with a passkey assertion (412 first),
  and takes the org's teams, invites and apps. Ownership moves with
  `SetOrgRole { role: owner }`; leaving is `RemoveOrgMember` on oneself.
- **Devices**: a person is their account, and each passkey is a device: the
  session names the one it was opened with (`SessionClaims::passkey`,
  `passkeyId` in `/auth/me`); `auth/passkeys/add` adds one.
- **Init**: `POST /apps` takes `initArgs` (JSON) for the app's `init`, kept on the
  `AppRecord`; a failed init answers `422 INIT_FAILED` (`AppState::init_error`).
- **Identity inside the app**: `env::account_id` and `env::executor_id` are both the
  caller's account (no device ids); the context id is `SHA-256(app_id)`.
  `env::caller_org` is the caller's role and teams in the app's organization
  (`app::caller_org`, passed through `Module::run_cloud`).
- **Private storage** (`env::private_storage_*`): v1 kept it on the member's node,
  unsynced. Here it is the caller's own space in the app's state (`app::private_prefix`):
  replicated and sealed, but a run is handed only its caller's space.
- **Shared-storage writers**: a cell's rotated writer set is kept in the app's state
  (`app::writers_key`), written with the run that rotated it.
- **Ids people type** (accounts, apps, organizations, teams, upgrade runs, API
  keys) are base64url that never starts with `-` (`api::typed_id`), which
  meroctl would read as a flag. Regression: `a_typed_id_never_starts_with_a_dash`.
- **Context ids**: an app answers to its context id (`hex(SHA-256(app_id))`, what the app
  sees as `env::context_id()`) wherever the API takes an app id (`Node::canonical_app_id`).
- **Blobs** (`env::blob_*`, `POST/GET /apps/{app}/blobs[/{id}]`): bytes live in the
  cluster's blob store (`hex(SHA-256)`, a majority before the answer, a missing one
  fetched from a peer), and the app's state marks which are its own
  (`app::blob_key`). A run's blob host is `services::AppBlobs`: a run writes blobs
  onto its outcome (never through the host), the leader stores them before
  proposing and spreads them after the entry commits; a run opens only blobs its
  app's state marks. An upload commits `AppCmd::Blob`. Ids are base58 (what apps
  print, `BlobId`) or hex in the API. `GET /apps/{app}/blobs[/{id}/info]`
  list them from a scan of the app's state (`AppMachine::storage_stats`, on a
  replica). A download asks the app's leader whether the app still marks the
  blob (`blob_held_strong`; 503 when no leader answers): a replica that has not
  applied a delete still holds the mark and the bytes (regression:
  `a_blob_deleted_through_the_leader_is_not_served_by_a_follower_behind`). `DELETE /apps/{app}/blobs/{id}` (owner or org admin) commits
  `AppCmd::BlobForget`, then `Node::collect_blob` deletes the bytes on every
  node (`internal/blob/delete`) unless another app still marks the same content
  (asked of each app's leader, `blob_held_strong`: a lagging replica could miss
  a mark just committed; an app whose leader does not answer counts as holding
  it) or it is a bundle or a release `.mpk`. Known window: the same content
  uploaded to another app between that check and the deletion loses its bytes.
  A blob past `MAX_BLOB_BYTES` (47 MiB) is refused `413 TOO_LARGE` by the node,
  one past the sealed cap (64 MiB) by the envelope, in the clear; the client
  reports both as `TOO_LARGE`. Regressions:
  `a_blob_too_large_to_reach_the_other_nodes_is_refused_up_front`,
  `an_upload_past_the_sealed_cap_is_answered_413`.
- **App frontends** (`app_host.rs`): with `--apps-domain D`
  (`merod local`: `localhost`), each package's frontend is at
  `{label}.D` (`app_host::label`, 52 base32 characters), from its release's
  `.mpk` (`Node::release_mpk`, verified before a byte is served, opened ones
  cached). Outermost layer: only `GET`/`HEAD` of files, `/` → newest release,
  client routes and relative tails served as the bundle's, its CSP plus the cluster's API
  origins in `connect-src`, `/.well-known/calimero.json`; an unknown label under
  D is a 404, never a node route. App origins pass CORS, and a passkey ceremony
  from one opens a session bound to its package (`SessionClaims::app`, carried
  by refresh): `Principal::within` keeps it to that package's apps (gate, app
  reads and writes, listings), `unbound`/`holder` refuse it credentials, API
  keys, approvals, upgrades, uninstalls and uploads, and it creates instances
  of its own package only. Its step-ups come from its own origin only.
- **Consent page** (`consent_page.rs`; docs: protocol "The consent page"): an
  app (any session of the account, package-bound included) asks
  `POST /consent/requests { kind: release|upgrade|outbound, scope?, subject,
  returnTo }` (sealed; a release subject is `{ version, withdraw? }`,
  `withdraw: true` withdrawing its approval) and gets `{ requestId, url,
  expiresAt, code }` (`consent_page::confirmation_code`, shown on the page
  and by the console); `returnTo` is the package's own frontend origin or one its
  newest signed manifest names (unbound sessions: `config.origins`). The
  request (this node's memory,
  `CONSENT_REQUEST_TTL`, per-account and per-node bounds, decided once) freezes
  a model from node state and verified manifests (an upgrade's capability
  sheet is `consent_page::diff`; the plain-language `title`, `points` and
  `attention` the page leads with are the node's too, and the rest sits under
  a collapsed *Technical details*). `GET /consent/{id}` (static HTML in the
  binary, hash-only CSP, `frame-ancestors 'none'`), `/model`, `/approve`,
  `/decline` are an outer layer in the clear on the node's own origin (outside
  CORS and the sealed channel); `/approve` runs the same route code
  (`api::upgrade_app`, `set_release_approval`, `set_outbound_approval`) as a
  `Principal { consent_page: true }` for the request's account, so the
  `412 CONFIRM_REQUIRED` challenge is over the frozen arguments, and the
  step-up may come from the nodes' own origins (`consent_page::relying_party`).
  Upgrade consent always binds the target's manifest hash
  (`VerifiedBundle::manifest_hash`); a release approval always binds its
  measurements digest (`consent_page::release_digest`: Sigstore-verified,
  `503` when unfetchable, over no measurements on mock attestation); a
  withdrawal binds `{ version }` alone and fetches nothing
  (`ReleaseChange::Withdraw`); neither route takes the hash from the client.
  `GET /consent/{id}/result` (sealed, same account; forwarded to the holder
  over `internal/consent/result`).
  Package-bound sessions are still refused the approval routes themselves.
  Regressions: `tests/consent_page.rs`.
- **Presence** (`presence.rs`, the ephemeral awareness channel): in memory on
  every node, never in a log. `POST /apps/{app}/presence { seq, state }` (anyone
  who reaches the app; one entry per account and device) applies it here and
  passes it to every other node (`internal/presence`); `GET .../presence` and
  `GET .../presence/stream` (SSE: `snapshot`, then `presence` diffs). Rules:
  newest seq wins, a retract is a tombstone, a heartbeat (same seq and state)
  refreshes and is passed on too, `PRESENCE_TTL_MS` (7 s) sweep each second,
  `MAX_AUTHORS_PER_APP` (512, the stalest evicted), 4 KiB of state.
- **Inspection**:
  `GET /apps/{app}/storage` (keys, bytes, blobs), `GET /apps/{app}/abi` (the
  bundle's embedded ABI), `GET /auth/usage` (the account's apps, bundle bytes,
  API keys, owned orgs, and the quotas).
- **Search** (`services::Search`): tantivy indexes, in memory per
  app and built from the app's `__calimero_search_*` exports. A view brings them
  up to the very state it reads first: from the entities entries touched
  (`Outcome::touched`, from the run's `StorageDelta`; kept for the last
  `TOUCHED_RING` entries), else a full scan. Derived, never replicated.
- **Follow-ups** (`AppState::pending`): an entry's event handlers
  (`app::emit!((e, "handler"))`, `tee:` for a TEE handler) and its `xcall`s are
  queued when it applies; the leader runs them (`Node::drive`, woken by `kick`)
  as entries that consume them (`Trigger::FollowUp`), or, for an xcall, calls
  the target with an idempotency key and then commits `AppCmd::Done`. A new
  leader neither skips nor repeats one. Only `#[app::handler]`
  methods run for an event; an xcall reaches only `#[app::xcall]` entry points
  (`from_same_app`: the same bundle), only within the caller's organization
  (outside one: its owner), as `Caller::App` with `env::xcall_origin`; cascades
  stop at `MAX_FOLLOWUP_DEPTH`. Handlers and xcalls run once, not per replica.
- **Timers and the TEE**: the cluster is every app's TEE. The leader fires each
  `#[app::tee(every = "..")]` method once per period (`Trigger::Timer`, the
  last tick per method in `AppState::timer_ticks`). Such a run and a `tee:`
  handler author as `AccountId::TEE_AUTHORITY` with `tee_trigger`, open and
  seal with the app's TEE key (`backup::tee_key`, from K_c), and seal a
  member's share to the key the node keeps for them (`backup::member_key`):
  their account is their one device (`SealingContext::stand_ins`), and only
  their own runs open with it. ABI facts (timers, handlers, xcall entry
  points) come from the bundle's embedded ABI (`services::BundleFacts`), so
  apps are built with `cargo mero build`; a bundle without one gets none.
- **Teams** (`types::Org`): nested (`parent`, moved with
  `UpdateTeam`), open (`TeamJoin::Everyone`: whoever reaches the parent, the
  organization at the top, is a member, unless denied `caps::JOIN_OPEN_TEAMS`)
  or restricted (direct rows only). A direct row has a role (`TeamRole`: lead,
  member, read-only) and capabilities (`caps`, `MemberCapabilities` bits;
  the organization keeps a default). An app deployed with `access: team` is
  reached by the team's members; a member whose direct row there is read-only
  runs calls, but one that would write is refused (`403`, `node::READ_ONLY`;
  v1 discarded the writes). A team's creator leads it. Below admin, creating an app in the organization needs `caps::CREATE_APP` (on its team, or org-wide) and minting an invite `caps::INVITE_MEMBERS` (org-wide, or on each team it names); neither is in `caps::DEFAULT`, and a team's leads hold both there. `caps::MANAGE_APPLICATION` lets a member upgrade the org's apps (on the app's team, or org-wide), start org upgrade runs, and approve the org's outbound hosts (org-wide grants only); a personal app stays its owner's (`SystemState::manages_apps`; regressions: `a_member_needs_the_capability_to_create_apps_or_invite`, `manage_application_lets_a_member_upgrade_and_approve_outbound`). A member who left, or
  whom a manager removed, does not come back by inheritance until a manager adds them again or an invite names the team (`Team::removed`; regression: `leaving_an_open_team_sticks_until_someone_lets_you_back_in`). Reads are filtered: a
  team one neither reaches nor manages is not listed (`GET /orgs/{org}`) and
  answers `404` (`GET /orgs/{org}/teams/{team}`). `DeleteTeam` takes the teams
  below it and their apps. Metadata (`POST /orgs/{org}/teams/{team}`) is the
  app's (a folder's name and colour).
- **App state on disk** (`app.rs`): `AppMachine` holds no state in memory but its
  small `Head` (bundle, pending follow-ups, timers, ring bounds). The app's keys are
  hidden rows of its group (`rows::KV`), idempotency records too (`rows::IDEM`, expired
  oldest first from a ring per TTL class), events a ring of `EVENT_RING` (`rows::EVENT`).
  Runs read through `StateRead` (`KvView` for the app, a `BTreeMap` in tests); the
  system group stays in memory (`Machine::IN_MEMORY`). Backups (`Node::backup`) copy the
  group's sealed snapshot file; `backup::latest` opens one with K_c.
- **Fresh system reads**: `Node::sys_fresh` is a read index (the leader names
  its applied index, `internal/system/index`; this replica waits for it). Org
  and team reads use it, and so does an access check before it refuses.
  Concurrent callers share a round (`ReadRounds`): one that arrives while a
  round runs waits for the next one, never takes the one already under way.
- **Declared views**: a call (`/intents`) of a method the ABI declares `#[app::view]`
  runs as a strong view: it writes nothing and may search.
- **Browsers**: `config.origins` are the CORS origins too (`lib.rs`, outermost layer).
- **Storage key** (`kms.rs`): K_r = HKDF(KMS key, cluster). A TDX node takes it from
  mero-kms only (`kms::key_source`: `--storage-key`/`--storage-key-file` need `--dev`),
  fetched by merod itself (`--kms-url --kms-identity`; `calimero-kms-client`:
  release-policy-verified KMS, key sealed to the TD). The boot script uses the same
  client for the data disk's key (`merod kms disk-key`). The cluster's nodes share
  the disk identity, so the nodes of one release share K_r; every release has its
  own mero-kms root, so K_r changes at each release upgrade.
- **Cluster key** (`cluster_key.rs`): K_c seals the store, snapshots, backups and
  feeds the cluster secret, TEE, member and component-cache keys, so data
  outlives a release. Kept in `<data_dir>/cluster-key`, wrapped under K_r. The
  founder (`--bootstrap`) and a store from before K_c take K_c = K_r (nothing on
  disk changes). A node on an empty data directory (`run`, before `Node::start`)
  serves only `/health` (`waiting`), `/attestation` and
  `/internal/cluster-key/deliver` while it asks its `--peers`
  (`POST /internal/cluster-key { cluster, node, token }`, no cluster secret); a
  peer never answers with K_c but delivers it (`cluster_key::deliver`) to the
  address registered for that node id (or in its own `--peers`; not removed),
  over a fresh sealed channel attested under the peer policy, with the token
  (`MAX_CONCURRENT_KEY_RELEASES`). A K_r that does not unwrap the file stops the
  node: no in-place move to a new release. Regressions:
  `nodes_on_the_next_release_keep_the_data_of_the_ones_before`,
  `the_cluster_key_goes_only_to_the_address_of_a_node_the_cluster_knows`.
- **Peers' quotes** are judged like a client's (`calimero_client::Measurements`):
  in TDX mode the signed mero-tee release the peer names, profiles from
  `--peer-profiles` (default: this node's `MERO_TEE_PROFILE`), floor `--peer-min-release`.
- **Sessions are node-signed**: `api::session_token` mints `v2.<claims>.<sig>`
  with the node's receipt key (`Node::mint_session`); a node whose release may
  not serve the account answers `421 WRONG_NODE` instead; `api::verify_token`
  checks the token under the issuer's receipt key as this node attested it at
  the issuer's registered address (`Node::session_issuer`) and requires the
  attested release to serve the account (`SystemState::release_may_serve_account`);
  `Principal::issuer_release` then must be approved for each app it reaches
  (`Principal::within`). There is no shared session key. **Failover**: every
  node attests every registered peer's receipt key ahead of time
  (`Node::warm_issuers`, from a loop in `lib.rs` on each system change and every
  `ISSUER_WARM_EVERY`: at start, once the directory is learned, on
  registration; `MAX_CONCURRENT_ISSUER_ATTESTS` at once, each bounded by
  `ISSUER_ATTEST_WAIT`) and keeps `node → Issuer { key, release, at }`, so a
  token verifies with its issuer stopped or frozen. A token under another key
  attests the issuer again (at most every `ISSUER_RETRY_SECS`, bounded the same
  way); unreachable or another key: `401`, and the client refreshes on any node
  (`calimero-client` does once on its own). A removed node's entry is dropped and
  `session_issuer` refuses a node that is no longer a peer. The key survives a
  restart, so do the node's sessions. Wrapped under K_r rather than K_c: K_c is
  every release's, and a node on another release booted on the disk could sign
  as this one under the release the others attested. Regressions:
  `sessions_signed_by_an_unapproved_node_reach_nothing`,
  `a_session_outlives_the_node_that_issued_it`,
  `sessions_survive_a_restart_of_the_node_that_issued_them`,
  `a_removed_nodes_sessions_are_refused`.
- **Quotas** (`system::Quotas`, set with `POST /ops/quotas`) live in the system state and
  are checked at apply, so every node enforces the same limits; refusals are `429 QUOTA_EXCEEDED`.
  `maxFileBytesPerOrg` counts a component's file bytes: `SystemState::app_file_bytes` per app,
  `payer_file_bytes` is its sum per payer (`usage::payer`), kept in step by `set_app_file_bytes`, so `DeleteApp`, `DeleteOrg` and
  `DeleteTeam` free files through `remove_app`; `SystemCmd::ChargeFileBytes { app_id, delta }`
  refuses a charge past the quota and floors a refund at 0; `TransferApp` of a personal app is
  refused when the new owner's quota would not hold its files.
- **Read-your-writes across nodes**: lookups of system objects (`Node::sys_until`) wait
  up to `Config::read_wait` (2 s) for the local replica to apply a write committed
  elsewhere (`sys_fresh` waits as long, and says whether it caught up). A
  refresh is judged by the system log, not by a stale replica: a token this
  replica cannot find, or whose exchange the log refuses, goes to the log as
  `RefreshReused`, which revokes the session if the token was exchanged before
  and is refused if it never was (then a replica that is behind answers 503,
  never "unknown"). Regression:
  `a_reused_refresh_token_revokes_the_session_on_a_lagging_node`.

## Component executor (`calimero:app` components, the v2 SDK)

Spec `app-runtime-v0.14`, on the node's log,
API and system group. What differs from the wasmer path:

- **Engine and host** (`node/component.rs`): one `WasmtimeEngine` per node
  (`Node::app_engine`), compiled artifacts cached under `<data_dir>/components`, each
  with a MAC under a key derived from K_c (S8). `Node::prepared_for` prepares a bundle
  once (single-flight, Ruling 44; `concurrent_first_uses_of_a_bundle_prepare_it_once`).
  `POST /bundles` and package installs run the engine's `prepare` on a component
  where they compile a core module on wasmer.
- **State** (`component::ComponentState`, D46): the component's stored keys
  (generation prefix included, D39) are hidden rows of the group (`rows::CKV`, sealed
  and HMAC-keyed like a wasmer app's), and each replica keeps them in memory too as an
  ordered `Arc<BTreeMap>`: a run gets the current version as its `StateView` (ordered
  `range` scans, which hidden rows cannot answer); `apply` copies it only while a run
  still holds the previous one; `restore` (a restart, a snapshot install) reads it
  back from the rows. Host records (`ComponentHead`: `last_ts_ms`, `last_id`,
  `install_ts_ms`, `timers`, the idempotency rings) live in the group's `Head`.
  Idempotency (D74 in spec v0.26, `idempotency.rs`, the one rule of both executors):
  records in `rows::CIDEM` (wasmer: `rows::IDEM`), a ring per class in `rows::corder`
  (wasmer: `rows::expiry`). A key's class is its prefix (`class_of`): a `node:` key (the
  node's own, or a D70 client's `node:c-…`) is kept 10 minutes, a caller's own key 24
  hours. A record expires once the earlier of the newest applied entry's timestamp and
  the applying entry's own (`expiry_now`) passes its own by that time, never by a node's
  clock, so every replica drops the same records and one entry stamped by a skewed
  leader can't expire what follows it. At most 100,000 records per app: past the cap
  `node:` records go first, then the oldest caller record, so a burst of keyless calls
  never evicts a caller's own record; the cap never takes a class below its floor (the
  newest 10,000 caller keys and 1,000 node keys stay whatever their age). Each entry drops at most `EXPIRY_STEP`
  (4,096) records; the rest carries over. Tests: `idempotency_records_are_kept_by_time_class`,
  `the_idempotency_cap_drops_node_keys_first_then_the_oldest`, `replicas_drop_the_same_records`,
  `a_burst_of_node_keys_does_not_evict_a_callers_record_below_its_floor`,
  `a_burst_of_node_keys_does_not_evict_callers_records_above_their_floor`,
  `one_skewed_entry_does_not_expire_the_records_written_after_it`,
  `a_callers_key_containing_node_is_kept_24_hours`, and on wasmer the same with
  `_on_wasmer`, plus `records_over_the_new_cap_drain_a_bounded_number_per_entry`.
  **Upgrade note (rolling upgrade, no version gate):** while a cluster runs both the
  previous release and this one, an app's replicas keep different idempotency records:
  the previous release expires only when a keyed call applies, against that call's own
  timestamp, and caps 1,000,000 records per class; this one expires on every entry and
  caps 100,000 per app, draining any excess `EXPIRY_STEP` per entry. Records are never
  compared between replicas and app state is unaffected; what differs is how long a key
  is honoured, which is decided by the replica that leads the app when the retry
  arrives. Both keep a key for its class's time unless the app is over its cap, and
  both keep the newest records of each class (the floors) whatever their age, so a
  retry within D70's ~9 s window is answered either way. Finish upgrading an app's
  replicas before relying on the new cap.
- **Calls** (`Node::call_component`, routed by the API from the app's executor):
  inside `Group::propose_led`, off the async workers (`node::off_workers`, the one
  copy both executors use); `base_index` is the app's last entry
  (`AppState::last_index`), as on the wasmer path. Statuses (spec section 5, D23,
  D36): `422 APP_ERROR` with `details.{code, fields}`, `422 TRAP` with
  `details.{kind, detail}`, `503 TIMEOUT` (not logged, key not spent); arguments
  over `MAX_ARGS_BYTES` (512 KiB) are `413 ARGS_TOO_LARGE`, an entry over the log's
  cap is logged trapped `HostDenied` (D50). Answers carry the node's receipt.
- **ABI gate** (`Node::abi_gate`, `abi_check`, S5, Ruling 31/40): from the ABI of the
  bundle the group runs; default-deny; `/intents` reads `view` and `confirm` from it
  (`AppRecord::confirm_methods` is the wasmer path's). A compute method (kind `compute`) and a
  `computeResult` method are `WrongKind` on `/intents` and `/views` for every caller, System
  included: only the node runs them.
- **Unique ids** (D52): `id_term` is the term `propose_led`'s leadership check found
  (no round of its own), `id_seq` this node's per-term counter (`Node::next_id`),
  continuing from `max(memory, AppState last_id)`. Safety on this log: a deposed
  leader's entry never commits or applies as a no-op through `base_index`; a leader
  restored into its old term catches up with its own log once before its first
  proposal (`propose_led`), so the counter continues after every entry it appended,
  committed index saved or not. Tests:
  `unique_ids_survive_a_crash_before_the_commit_index_reaches_the_disk`
  (deterministic, through the log's test hooks), `unique_ids_survive_a_restart_into_the_same_term`,
  `unique_ids_continue_when_a_leader_restarts_into_its_term_in_a_three_replica_group`,
  `a_snapshot_install_carries_a_components_host_records`.
- **Time and timers** (D16, D19, D34): `Node::app_clock` (`Config::clock_offset_ms`,
  `Node::advance_clock` under feature `testing`); `lib.rs` polls `node::TIMER_POLL`
  and `Node::tick_timers` proposes each due slot (`ComponentCmd::Tick`) as
  `Caller::System`; a slot at or before the applied one changes nothing.
- **Events** (R9): a component's `EventRecord` carries its `audience` and caller;
  the event stream (`api::local_events`, the subscriber passed to a peer's
  `internal/events`) and a call's answer check `SystemState::receives` when they
  send it, so a removed member stops receiving at once. A wasmer app's events have
  no audience and go out as before.
- **Upgrades**: the node's route and authority (`POST /apps/{app}/upgrade`,
  passkey consent, `UpgradeApp`); at the leader a component upgrades code-only
  (`componentUpgrade`) and a named `migrate` is refused `MigrationsNotSupported`.
- **Identity a component sees** (`component::SysDirectory`): its org, owner, the
  caller's role and teams there; `orgs-of` answers only the app's own organization
  (D75, spec v0.26): for an account, at most the app's org with its role, and nothing
  for a personal app. Test: `a_component_sees_no_organization_but_its_own`.
- **Per-executor endpoints** (`the_abi_storage_and_blob_routes_answer_for_a_component`):
  `/apps/{app}/abi` answers the ABI the component embeds (`calimero:abi`);
  `/apps/{app}/storage` counts its keys and bytes; the blob routes are its files' (see
  "Files": upload, list, info and delete work (list and info need the upload
  authority, as they show plaintext hashes), download answers `409 NOT_SUPPORTED`).
  The host refuses blobs, search, xcall, event handlers,
  TEE runs and outbound HTTPS to a component; a wasmer app's
  xcall into a component is refused (`node::COMPONENT_REFUSES`).
- **Files** (`node/component/files.rs`; tests `component_files.rs`): a run's `files`
  provider (`NodeFiles`) is built per run from the app id, the file key
  (`backup::file_key`, not the TEE key) and the app's marks (`ComponentState::files`,
  in memory like `kv`, rows `rows::CFILE` keyed by blob id, so a snapshot and `restore`
  carry them). A writer seals into memory (`FileSealer`) and wraps the data key;
  the blob id is the hex SHA-256 of the ciphertext. The outcome and the entry carry marks
  only (`Outcome::files`, `deleted_files`, serde defaults: old entries decode unchanged);
  `apply` writes or removes them only for status `Ok`; a replica that applies a delete drops
  the bytes from its own store once the entry shows as applied (`AppMachine::forgotten`),
  so no one else deletes after a commit. Before a call, tick or `init` proposes,
  `Node::persist_files` stores the ciphertext here and pushes all new files at once to the
  app group's voters and learners (its own Raft membership, not the directory's replicas),
  under one deadline below `FORWARD_TIMEOUT` and a node-wide byte budget (`MAX_PUSH_BYTES`).
  It needs a majority of the voters, this node counted; a voter with no address still counts.
  It runs only after the entry bytes are built, so an entry the log replaces as too large
  stores nothing. When the majority is missing the call is `503 FILES_UNAVAILABLE` (the
  forward loops pass it on unchanged and do not retry it; the client does, with its key),
  nothing is logged and the pushed bytes are removed; so are the bytes of an entry that
  conflicts or whose proposal fails. `open` uses the local blob only if its SHA-256 is its id,
  else drops it and fetches it from the other replicas (5 s), and decrypts as the guest reads;
  any opener failure, including at the end of the stream, is `ReadError::Corrupt`, which the
  host core makes a `HostDenied` trap. Open readers hold their ciphertext bytes against a
  per-run budget (`MAX_RUN_OPEN_BYTES`) and a node-wide one (`MAX_OPEN_FILE_BYTES`; logged
  runs wait 1 s for it). A file that cannot be had (no replica holds it, or a budget is
  spent) aborts a logged run as `503 TIMEOUT`, not logged, key not spent
  (`FileError::Unavailable`, also for `create`); a view gets the soft error
  `"file bytes are not available"`. The provider is built for every run but touches keys,
  budgets and bytes only when a run touches a file. Blocking on the runtime from a run needs a
  multi-thread runtime, else the run is refused. Pushes skip members `may_send` refuses, and
  `fetch_blob_from` asks only the app's other replicas, which serve a file as a blob their app
  marks (`holds_blob`); bundles and wasmer blobs still use every node. A component's files count
  in `storage_stats().blob_bytes` as stored (a tag per chunk).
  Replica moves hand over the marked files with the bundle (`hand_over_bytes`), and again
  after the voters change, only what the joining nodes do not hold, for files made while the
  move ran; the second hand-over is retried for 10 s and never abandons the move.
  Quota: a logged entry's new files are charged first, within `CHARGE_WAIT` (`Node::charge_files`,
  from the run's thread; refused: nothing is pushed, the call is `429`, nothing logged; a charge
  that lands after the wait is refunded when its answer arrives), undone by `Node::discard` if
  the entry does not commit. Refunds after a commit come from `Applied::freed`, the bytes of
  marks that entry really removed, so a racing delete never refunds twice. **Uploads** reuse the
  blob routes: a node that does not lead the app seals and stores nothing, it passes the body to
  the leader (`internal/file-upload`, one hop: a follower answers `421 NOT_LEADER` with
  `details.leaderUrl` and the receiver follows it), which (`Node::upload_local`) seals through
  `NodeFiles::create`, charges, pushes to its group's voters and commits `ComponentCmd::File`
  under a key made per HTTP request (the blob route takes no client key), so only a forward
  retried inside that request replays the first answer; a recorded answer for another file
  wins and the new file is undone. If the commit fails, a `componentBarrier` entry (applies as
  nothing) is committed through the leader, so the failed entry has either committed or never
  will, and the leader then reads the mark: marked, the upload succeeded; not, bytes and
  charge go; no barrier possible, they stay. Only the app's owner and organization members
  of role Member or above may upload.
  Delete (`forget_file`) is the host command `__file_forget` (`ComponentCmd::FileForget`, in
  `HOST_METHODS`, so `/intents` refuses it as reserved), idempotent under its key too.
- **Compute runs** (`component/runs.rs`; tests `component/tests/runs.rs`, `system/queue_tests.rs`,
  `component_compute.rs`): a call, timer, `init` or result method starts a run with `compute.start`;
  the host puts `Outcome::compute_starts` (and `compute_cancels`) in its outcome, the node fills
  the app's `owner` (`Outcome::name_owner`; the payer is the leader's to read from fresh system state when it acquires), and `apply` (status `Ok` only) writes a `RunRecord` per
  run as a hidden row (`rows::CRUN`, by run id) and adds its id to `ComponentHead::active_runs`
  (at most `MAX_ACTIVE_RUNS`; a start past it or of a known id is dropped). `finished_runs` keeps
  the last `FINISHED_RUNS_KEPT` (256) finished ids, oldest first; a record leaves with its id.
  The record pins the bundle that ran at start (an upgrade does not move it). Two more entries:
  `computeAttempt { run, attempt, node, jobKey, assignmentDigest, tsMs }` applies to an active
  run whose cancel was not requested, when `attempt` is the next number and at most `MAX_ATTEMPTS`
  (3) and both keys are hashes (64 lowercase hex); it sets `running`. `computeFinal { run, attempt,
  outcome, report?, signature?, outputs, usage, result, baseIndex, tsMs, idTerm, idSeq }` ends it:
  the first valid one wins and any later one is a no-op. A job outcome (`done`, or `failed` with
  `fuel | memory | time | trap | app-error | integrity | cancelled`) needs a report and signature that
  `verify_report` accepts under the latest attempt's `jobKey` and `assignmentDigest`, for this app,
  run and attempt, with the same status and reason, and `outputs` and `usage` exactly the report's.
  A `cancelled` final (or `failed cancelled`) needs `cancelRequested`, and is either the leader's
  (no report, zero usage) or carries the job node's report (status failed, reason cancelled) that
  verifies like any other, with its usage. Other leader outcomes carry no report, no outputs and
  zero usage: `failed lost` needs every attempt used, `failed quota` (the outputs passed the payer's file quota), `failed timeout` a run that was assigned,
  `failed refused` (an acquire refused for good) a run with no attempt. Outputs are bounded
  (`MAX_NEW_FILES`, `MAX_FILE_BYTES` each, `MAX_COMPUTE_FILE_BYTES` together) and a job's usage
  cannot pass its caps (fuel; CPU micros and memory-GiB-micros at most the wall cap plus
  `DEADLINE_GRACE`, 60 s). A queued run whose fuel exceeds a lowered `computeFuelPerOrg` is marked
  `RunSlot::refused`; the leader ends it `failed refused`.
  `attempt` must be the run's latest (0 before any).
  An output id that is already a file, or listed twice, is refused. Every permanent refusal changes
  nothing and answers `Applied::refused` (status `AppError`, the reason as `error`); only a stale
  `baseIndex` answers `conflict` (retry on the current state) and leaves the run active. `init`
  starts runs on its own bundle. The result method runs with `runs_provider(Some(run))`, which
  leaves out the run that is ending, so its own start of the same job is a new run. `new_file_bytes`
  counts a final's result method's new files only (`None` on overflow refuses the entry): the
  leader stores and charges the outputs before it takes the proposal lock. A final over the log's
  cap loses only its result method's outcome; if it is over the cap without it the entry is not
  built (`FINAL_TOO_LARGE`, the leader ends the run `Failed{timeout}`). An admin file delete (`FileForget`)
  of an input of an active run is refused like a call's (`FILE_IS_RUN_INPUT`); a run record that
  cannot be read protects its inputs and makes the host deny start, cancel and delete
  (`RUN_RECORDS_UNREADABLE`). A run being cancelled is not the answer to a start of the same job. Applying marks the
  outputs whatever the result method did, records the final, frees the run's slot, then applies
  `result` as a call's outcome (only if its status is `Ok`; its own starts and cancels included).
  The system group queues runs (`SystemState::compute`, `RunSlot`): `AcquireRun` (idempotent;
  payer must be the app's, fuel at most `MAX_COMPUTE_FUEL`) and `ReleaseRun` run the grant pass:
  capacity is job nodes times `RUNS_PER_JOB_NODE` (2); the payer whose oldest waiting run has the
  smallest `seq` is served first, and a payer at `maxConcurrentRunsPerOrg` (4) or over
  `computeFuelPerOrg` (10 x `MAX_COMPUTE_FUEL`) is passed over for the pass. `RegisterNode`,
  `RemoveNode`, `SetQuotas`, `TransferApp` (a personal app's payer moves) and `remove_app`
  (`DeleteApp`, `DeleteOrg`, `DeleteTeam`) re-run it; a granted run is never taken back.
  `AcquireRun` answers `{ state: granted | queued | refused, code, retryable }` (retryable: `NOT_PAYER`, `APP_QUEUE_FULL`; permanent: `NO_SUCH_APP`, `FUEL_OVER_RUN_MAX`, `FUEL_OVER_QUOTA`, `RELEASED`); it refuses a run id released before (`released_runs`, the last 256 per app),
  past `MAX_ACTIVE_RUNS` runs queued for the app, and for a run whose fuel alone exceeds
  `computeFuelPerOrg`. `AcquireRun` and `ReleaseRun` are proposed only once a job node is
  registered, which already requires every system node to be on this release.
  `computeStarts` and `computeCancels` follow the rule files do: a node never moves to a new release
  in place (its storage key is per release), so an older replica that would miss run records is
  retired with its release, and an older node cannot prepare a bundle with compute methods
  (unknown kind, forbidden import).
  `storage_stats` lists a component's marks as its blobs (with plaintext hashes), which the
  list and info routes read. A node that stops hosting a deleted app drops its file bytes.
  **Upgrade note (quota):** an older system leader rejects `ChargeFileBytes`, so uploads and
  file-creating calls fail until the system group runs this release; a snapshot from before
  carries no file-byte counts, so files stored earlier start from zero.
  **Upgrade note (rolling upgrade):** a node on a release without files refuses to prepare a
  bundle that imports `calimero:app/files` (`ForbiddenImport`), so it never runs, leads or
  answers for an app that uses files. Replicas of different releases coexist only while a
  release upgrade moves apps, and the old ones are retired.
- **Driving compute runs** (`node/component/compute.rs`, `compute/{job_io,finish}.rs`; tests `compute_runs.rs`): `lib.rs` calls `Node::drive_runs` every `JOB_POLL` (2 s) and when a call commits (`runs_kick`); for each component app this node leads, one task per app frees stray slots and starts a step task for each active run not already being stepped (one step per run at a time, so a run stuck in a long transfer never delays another run's step or cancel; a step that finds this node no longer leads its term does nothing) from the replicated record and the system state, so a new leader resumes where the last stopped. What it keeps in memory (`Watch`, per run and leadership term) is one healthy-time accumulator (time in rounds whose system read succeeded: the lost clock, the run deadline and `QUEUED_RUN_TIMEOUT` count only it, so a system-group outage burns none of them), when a job node last answered in that time (a new leader starts the clock at takeover), the valid report a job node gave (held per attempt, so the leader's own failing charge, push or propose never counts as silence; a job node that will not hand its outputs over still does; outage credit is capped at `MAX_OUTAGE_CREDIT` (`QUEUED_RUN_TIMEOUT`), the lost and cancel clocks also end at 10 times their limit in wall time, and a running run ends by timeout at an absolute ceiling of wall cap + `DEADLINE_GRACE` + `MAX_OUTAGE_CREDIT` on the app clock from the attempt's `tsMs` (it survives leader changes), whatever the leader sees; the unplaced and cancel clocks start when the current leader first sees the run in that state and restart with each term: unplaced `QUEUED_RUN_TIMEOUT` of healthy time, at most `QUEUED_RUN_TIMEOUT` + `MAX_OUTAGE_CREDIT` of app time, cancel `CANCEL_DEADLINE` of healthy time, at most 10 times that in wall time; a loss the job node announces (`lost`, a new key, `UNKNOWN_RUN` after sending) first finishes from a held report, `lost_unless_held`), whether this leader sent the attempt, the last progress and the outputs staged for a final. A queued run: `AcquireRun` (only when a job node is registered; the payer comes from fresh system state; a permanent refusal ends the run `Failed{refused}`, a retryable one waits), then, once granted, `Node::choose_job` (each job node attested and asked for its load; fewest runs then lowest id; a node that lost an attempt only if no other is free), the assignment is built and delivered (component, assignment, inputs) to that node first, and only then is the attempt committed with the job key and the digest, so a busy or unreachable node (503 is not a failed channel) spends no attempt and the next candidate is tried; a delivered run whose attempt did not commit is deleted. Choosing and reserving a place is serialized (`assigning`, `reserved`), the transfers are not. A granted run that no job node takes for `QUEUED_RUN_TIMEOUT` (10 min) ends `Failed{refused}`. The assignment is a pure function of the record, the marks and the job node's keys (`build_assignment`, `wrap_for_job_derived` derives the wrap's ephemeral key and nonce from the file key), so a new leader rebuilds the very bytes the digest names; a digest or job key that differs means the attempt is lost. A run polls `GET /job/runs/{app}/{run}/{attempt}`: `running` is the heartbeat, `404 UNKNOWN_RUN` or `awaiting` makes the leader send (component on `NEED_BUNDLE`, assignment, inputs) unless it already sent in this leadership (then lost), no answer for `JOB_LOST_AFTER` (20 s; before giving up the leader looks at the record, a held report and one fresh poll, `rescue`), `lost`, or a report that does not hold up (signature, usage over the caps, outputs over the limits, a cancel nobody asked for) is a lost attempt: the next attempt goes to another job node, at `MAX_ATTEMPTS` the run ends `Failed{lost}`. A done report is verified with `verify_report`, its outputs are charged (`QUOTA_EXCEEDED` ends the run `Failed{quota}`), then fetched and stored one at a time: SHA-256 of the ciphertext must be the report's id and it must open under the fetched key to the reported size and hash, else the attempt is lost. The final entry runs the result method inside `propose_led` (`runs_provider` leaving out the ending run; args are the outcome JSON, the SDK's `ComputeOutcome`) and is built by `logged_entry`; a final answered `conflict` is run again, one the record shows still active after commit is refused for good (a reported one makes the attempt lost, its staged outputs are discarded). After a final commits the leader releases the slot, deletes the job run and adds the usage to `calimero_app_compute_*` and `calimero_app_calls_total{kind="compute"}`; slots whose run is no longer active are released each round. A cancel first takes a report the job already has (a `done` run keeps its outputs), else deletes the job run and ends the run `Cancelled` with the usage in the job node's signed cancelled report, or with none if it does not answer; a run that did not stop is deleted again next round, but `CANCEL_DEADLINE` (20 s of healthy time from the first sight of the cancel) ends it: from the held report if any, else `Cancelled` with no usage, then the usual last delete (later reports are refused, first final wins); no final report by the attempt's `tsMs` + `wallSecs` + `DEADLINE_GRACE`, less this leader's outages, ends it `Failed{timeout}`. Every job-channel failure goes through `forget_job`; `job_call` runs every control request (status poll, delete, load, output key) in its own pool (`MAX_INFLIGHT_JOB_CONTROL`) within `JOB_CONTROL_TIMEOUT` (5 s) for the call, and a wait of that long for a place is `BusyHere`: it is this node's queue, not the job node's silence (the poll is `Deferred`, nothing is forgotten), and the attestation fetch has `JOB_ATTEST_TIMEOUT` (10 s), so a frozen job node cannot stall a round or starve control calls; `job_transfer` (assignment, component, inputs, output bodies) has its own pool (`MAX_INFLIGHT_JOB_OPS`) and 300 s; a cancel whose poll gets no answer ends from the held report if any (while that report only waits on the leader's own writes the run stays open, up to the run ceiling), else cancelled with no usage, without a delete. Staged outputs are undone (refund, delete) on every exit but the commit of the final that holds them, and a sweep (`sweep_watches`) undoes those of ended runs the app's state does not mark and counts a final whose proposal's answer was lost; a final the state refused for good is not proposed again until the record changes. The run route shows plaintext sizes and hashes of files only to those who may list the files. `GET /apps/{app}/runs/{run}` and `GET /apps/{app}/runs` (a page, `AppMachine::runs_page`) are the reserved view `__run` answered by the leader (`Node::run_view`).
- **Not here**: the spec's `__upgrade` through `/intents` (Ruling 36; the node refuses
  every `__` method there) and the leader's own re-check of upgrade authority
  (Ruling 45; the node judges it in the API and the system log); their tests are
  `#[ignore]`d with the reason.

## Not done yet

rollback counter (proposed: a mero-kms epoch, restored nodes rejoin as learners),
keys per app from mero-kms, snapshot restore into a
running cluster, event audiences, search indexes that survive a restart (rebuilt from state now).

## Commands

```bash
cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release   # test fixture
# fixtures whose ABI the node reads (handlers, timers, xcall, search, blobs):
for a in blobs kv-store-with-handlers kv-store-with-shared-storage search-chat xcall-example tee-cards tee-dice \
  migrations/migration-suite-v1 migrations/migration-suite-v2-add-field \
  migrations/scenario-migration-check-fail-v1 migrations/scenario-migration-check-fail-v2 \
  migrations/scenario-identity-downgrade-v1 migrations/scenario-identity-downgrade-v2; do
  cargo mero build --manifest-path apps/$a/Cargo.toml; done
cargo test -p calimero-node    # 3-node cluster in process, sealed, real WASM
# component fixtures (apps/v2-fixtures/{kv,org-gated}, apps/rooms) are built on first use by
# tests/common/mod.rs into target/app-components (needs the wasm32-wasip2 target)
cargo test --release -p calimero-node --test rooms   # Rooms A1 (shared scenario), A3, A5
# A4, the fault test (merod run --dev processes on ports 7600-7699, above the merobox
# workflows', shifted by WORKFLOW_PORT_OFFSET when set; release merod and
# meroctl in RV2_BIN_DIR, default target/release; the cached Rooms component)
python3 -I crates/node/tests/fault/rooms_fault.py --iterations 200 --seed 1
# compute actions end to end: 3 replicas and 2 job nodes as processes (same binaries, ports 7650-7655)
python3 -I crates/node/tests/fault/compute_e2e.py
# calls/s and p50/p99 through that cluster (harness: tests/harness/cluster.rs), as JSON:
cargo bench -p calimero-node --bench cluster_throughput -- --secs 15 --out cluster.json
cargo run -p merod -- --help
cargo run -p merod -- local   # dev cluster: 3 nodes on :2501-2503 (`local.rs`)
cargo run -p merod -- --node n1 inspect [--storage-key-file F]   # a stopped node's store (`inspect.rs`)
```
