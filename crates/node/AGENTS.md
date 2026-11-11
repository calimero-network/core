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
- **One engine** (`types::Executor`): an app is a `calimero:app` component (built
  with `calimero-sdk`) and runs on wasmtime through the host core
  (`node/component.rs`, `component.rs`). The executor is decided once, from the WASM
  the app's bundle names (a package's service or a raw upload,
  `wasmparser::Parser::is_component`), when the app is created (`AppRecord::executor`),
  and the app's `init` entry records it in the group's state (`AppState::executor`, in
  `Head`, so a snapshot carries it).
- **SDK 0.12 apps (wasmer) are gone**: the node no longer runs a WASM core module.
  `Executor::Wasmer` stays only so records and states written before still read (no
  recorded executor means wasmer). A core-module bundle is refused at upload, create and
  package install with `400 UNSUPPORTED_BUNDLE` (`ApiError::unsupported_bundle`), and an
  upgrade to one with `UNSUPPORTED_BUNDLE`; an existing wasmer app is still listed and
  deletable, and every call, view, upgrade, intent, xcall list and ABI read on it answers
  `410 UNSUPPORTED_ENGINE` (`ApiError::unsupported_engine`, `app::WASMER_GONE`). A wasmer
  entry in an app's log applies as a deterministic no-op (`AppMachine::unsupported`:
  `last_index` moves, nothing else). Tests: `old_datadir.rs` (a directory `a09547b2b`
  wrote), `a_core_module_bundle_is_refused_as_an_sdk_0_12_app`,
  `an_sdk_0_12_apps_entries_apply_as_no_ops`, `a_malformed_entry_applies_as_a_no_op`,
  `a_wasmer_entry_never_applies_to_a_component_app`.
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
  quote's release with it). That attestation, and the one before a channel to
  a peer opens (`Node::connected`), runs in a task of its own (`node::detach`,
  one per caller or peer at a time, `MAX_DETACHED`): on TDX it takes about a
  second, longer than the one heartbeat openraft gives an RPC, and dropped
  with the RPC it never finished (regression:
  `peers_that_attest_slower_than_a_heartbeat_still_form_a_cluster`). `api::peer` replaces the bare cluster-secret check
  and refuses an anonymous session. Routes: app data
  (`view`, `call`, `events`, `storage`, `last-write`, `blob/held`, `app-ready`, `presence`)
  need a release approved for the app (`caller_may_hold`); `blob/get` code to any
  node, other blobs only for an approved hosted app that marks them; `blob/delete`
  refused while a hosted app marks the blob or its leader is committing an entry that stored it; `raft` needs `rpc.sender()` = the
  caller and, for an app group, an approved release and membership (replicas or
  the group's voters/learners): openraft itself never checks (F4). Outbound,
  blobs (`spread_blob`) and presence go only to peers `may_send` allows.
  No interop with nodes before this (a hard cutover: clusters are recreated).
  Regression (`tests/approval_bypasses.rs`):
  `a_node_on_an_unapproved_release_reaches_no_app_through_internal_routes`.
- **Raft addressee** (`PeerTransport::send`, `api::internal_raft`): the sender check above
  covers one end of a Raft message; the other is its addressee. Every Raft message names the
  node it is for (`target`), and a node that is not that node refuses it with `421 NOT_ADDRESSEE`.
  So a genuine member's message that reaches the wrong node (a reused port, a node moved or
  replaced) cannot move that node's term or log. Regression:
  `a_raft_message_for_another_node_is_refused_and_the_group_stays_healthy`. Both cases are counted
  in `calimero_raft_misaddressed_total{reason="not_addressee"|"no_target"}`. **Transition:** a
  message without `target` comes from a peer on a release before this one (rc.4 and earlier). This
  release accepts it, counts it as `no_target`, and warns at most once per
  `api::NO_TARGET_WARN_EVERY` (60 s; one slot per process, taken by compare-and-swap; the count in
  between is in the metric). The release after this one refuses a missing `target` too
  (`RaftBody::target` stops being optional). Ship that only once every node runs this release,
  which `no_target` at zero on every node shows.
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
  `after: null` lifts it). Credentials and memberships carry the person's
  consent too (`account:<a>`: `__add_passkey { credentialId, publicKey }`,
  signed by a passkey the account already has; `__remove_passkey
  { credentialId }`; `__create_api_key`, `system::api_key_args`, the key's
  hash included; `org:<org>`: `__set_org_role { account, role }`,
  `__remove_member { account }`, `__set_team_member { team, account, lead,
  role, capabilities }`, `__remove_team_member { team, account }` (it takes
  the rows below the team too, `RemoveTeamMember { subtree }`),
  `__delete_team { team, apps }` (only when the team's subtree holds apps,
  `system::team_apps`; `DeleteTeam { apps_need_consent }`),
  `__set_capabilities { team, account, capabilities }`,
  `__set_default_capabilities { capabilities }` (a team's: `{ team,
  capabilities }`, `system::default_capabilities_args`), `__create_invite`,
  `system::invite_args`, `__revoke_invite { inviteKey }`, `__scope_apps
  { apps, shared }`, `system::scope_apps_args`). A step-up whose arguments the node makes (an API
  key, an invite token, the registered credential) goes through
  `api::consent_minted`: the first request keeps them, and what the retry
  needs but must not log (`Pending::Confirm::carry`), with the challenge.
  Invites are kept under the token's Ed25519 public key
  (`proof::invite_key`); `RedeemInvite` carries the token key's signature
  (`proof::sign_invite_redeem`), so a node that only saw the log cannot
  redeem one. A redemption that changes nothing for the account (a member at
  the invite's role or above, in every team it names) spends no use
  (`RedeemInvite { free_when_unchanged }`, which the API always sets; absent in
  an entry an earlier release logged, which replays spending one; regressions:
  `a_member_redeeming_an_invite_leaves_its_use_for_the_invitee`,
  `a_redemption_logged_before_free_redemptions_replays_as_it_did`,
  `an_owner_opening_their_own_invite_leaves_it_for_the_invitee`). The fleet
  carries the operator's signature (`cluster /
  __register_node { id, addr, zone }`, `__remove_node { id }`,
  `__set_quotas { quotas }`, `__set_pools { pools }`, `__set_price_table { table }`, `__set_org_quotas { payer, quotas }`; `<app> / __set_replicas { replicas }`, one
  signature for both of a move's writes: the union applies with it unspent,
  the target spends it). The configured peers come with `Genesis { nodes }`;
  `reconcile` registers nobody. The consent page (`consent_page::act`) logs the same
  consent as the API, its assertion made on the node's own origin (`apply`
  checks the signature and the statement, not the origin). Tests run
  commands' own authority rules through `run_authorized` (`run_unproven` with
  the new-work refusal). Test clusters start with the harness's
  operator key (`ops_signed` and `ops_signed_delete` sign like mdma). Regressions: `forged_decisions_are_refused_where_they_apply_on_every_node`,
  `forged_credentials_memberships_and_redemptions_are_refused`,
  `operator_decisions_need_the_operator_signature`,
  `credentials_memberships_and_the_fleet_are_approved_where_they_are_made`,
  `system::tests` (`credentials_need_the_accounts_own_passkey`,
  `memberships_need_the_actors_passkey`, `invites_are_redeemed_with_the_tokens_key`,
  `the_fleet_and_quotas_need_the_operators_signature`,
  `a_replica_move_is_signed_once_over_its_target`).
  Sessions too: a sign-up (`CreateAccount`) carries the new passkey's
  assertion over `system::account_args` (`account:<id> / __create_account`;
  `register/finish` answers `412` once the registration verifies, the pending
  `Pending::SignUp` keeps the account and the first refresh token, and the
  retry's `confirm` logs it); a sign-in's `CreateRefresh` carries the login
  assertion, whose challenge is that of `session / __sign_in /
  system::sign_in_args` (`api::NewSession`, made in `login_begin`); a
  refresh token stands for an Ed25519 key (`proof::refresh_key`, kept in
  `RefreshRecord::key`) that signs its exchange (`UseRefresh::signature`,
  `proof::refresh_use_message`; a record without a key is not exchanged);
  `SetSignCount` carries the assertion whose counter it records
  (`calimero_identity::assertion_counter`); `RevokeApiKey` the account's
  consent (`__revoke_api_key { keyId }`). Regressions:
  `forged_sessions_counters_key_revocations_and_accounts_are_refused`,
  `system::tests` (`a_sign_ins_refresh_token_needs_its_passkeys_assertion`,
  `a_refresh_token_is_exchanged_with_its_own_key`,
  `an_account_is_created_by_its_own_passkey`,
  `an_api_key_is_revoked_with_its_accounts_passkey`,
  `a_counter_moves_with_the_assertion_that_moved_it`). A cluster requires
  these session proofs from `SystemState::session_proofs_from` on: a genesis
  this release logs (`Genesis { session_proofs: true }`), or in a cluster begun
  before, the `RequireSessionProofs` its first system leader on this release
  writes (`reconcile`). An entry before it (what an earlier release logged,
  replayed on restart) applies as it did, so replicas never diverge on it
  (`session_commands_logged_before_their_proofs_apply_as_they_did`,
  `old_datadir.rs`).
- **API** (`api.rs`), all inside the sealed channel except `/attestation`, `/health` and the consent page:
  `/auth/*` (passkeys, refresh, API keys), `/bundles`, `/apps`, `/apps/{app}/{intents,views,events,upgrade,access}`,
  `/orgs/*`, `/invites/{token}/redeem`, `/consent/*`, `/internal/*` (peers), `/ops/*` (mdma: counts, placement and version reports by id only, no read path into apps).
- **Reserved methods** (`node::reserved_method`, `__*`): the node's own commands
  (`HOST_METHODS`: `__upgrade`, `__blob`, `__blob_forget`, `__blob_revoke`) run only for a
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
  A call whose arguments pass `MAX_ARGS_BYTES` (below that cap) is refused where it arrives
  (`413 ARGS_TOO_LARGE`), never proposed (regression:
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
  A service may also ship a compute component (`compute` in the manifest): it is prepared on
  `Node::compute_engine` only (never the replica engine), and the release is refused
  `400 COMPUTE_ABI_MISMATCH` unless its ABI lists exactly the main ABI's compute methods,
  each with the same caps, and `400 BAD_BUNDLE` when it imports outside the compute world or
  its ABI is not the compute world's.
  A release without one installs and its app runs calls; it cannot run compute.
  A single-service package may ship the compute component alone (no `wasm`): `PackageVersion::components` is `ComputeOnly`, `services` is empty and `compute[""]` holds the hash.
  Install skips the ABI agreement and instead needs at least one compute method (`400 NO_COMPUTE_METHODS`).
  It is installed only once every node runs this release (`409 CLUSTER_NOT_UPGRADED`, `require_upgraded_nodes`): an older node drops `components` and would apply it as an app's release.
  Every app-level lookup reads `PackageVersion::app_parts` (the services, or the compute map when compute-only), so a compute-only app's `bundle` and `compute_bundle` are both the compute hash.
  Such an app has `AppRecord::components = ComputeOnly`: it has no app group state, and a call, view, intent, init or tick is `422 NO_APP_COMPONENT` (`AppRecord::require_app_component`).
  An upgrade between compute-only releases only moves the directory record; one that adds or drops the app component is `409 COMPONENTS_CHANGED`.
  A release may ship pipelines (`pipelines` in the manifest or a service; `packages/pipelines.rs`).
  Install compiles each (`calimero_pipeline::compile`) and checks it against an `Env` with the module `self` (the package's compute methods and caps; the default for a step that names none), one module per other package a step names (`module: <package>[@<version>]`, resolved by `Node::resolve_modules` to a release the installer sees, newest when no version, that ships one compute component, else `UNKNOWN_MODULE`), and the cluster's pool memory ceilings; anything else is `400 BAD_PIPELINE` with `details: [{file, path, code, message}]` for every error, a package with pipelines but no compute component included (`NO_COMPUTE`) and a repeated name (`DUPLICATE_NAME`).
  `PackageVersion::pipelines` maps name to `PipelineRef { hash, worstCaseFuel }` (hash of the canonical JSON, fuel a decimal string, `modules`: reference as written to `ModulePin { package, version, compute }`, pinned at install so a newer module release moves nothing until the pipeline's package is released again; `InstallPackage` applies only while every pinned release is still installed, and `UninstallPackage` of a pinned release is `409 MODULE_IN_USE`, `SystemState::module_pinned`); the canonical JSON is stored as a blob by its hash beside the wasm parts, and `Node::pipeline_json(hash)` reads it, else recompiles it from the release's `.mpk`.
  Such a release is installed only once every node runs this release (`409 CLUSTER_NOT_UPGRADED`).
  `GET /packages/{package}/{version}/pipelines` (who sees the release) answers `{ name: { hash, worstCaseFuel, modules, pipeline } }`; `meroctl app pipelines`.
  `PackageVersion::compute` maps service to that hash, whose `BundleRecord` has
  `PackageRef::compute = true`: never a service (`resolve_release`, `CreateApp` and
  `UpgradeApp` refuse it), and fetched by `release_wasm` like a service.
  `AppRecord::compute_bundle` carries the running release's compute hash next to `bundle`.
- **An instance's release** (`AppRecord::release`, a `system::PackageRef`): the
  package release it runs, set at create (the version named, the newest visible
  one, or the deployment's offer) and by every `UpgradeApp` (one app, an org or
  publisher run, the operator's, the consent page's), which also checks that the
  release ships the bundle (`SystemState::release_ships`). A frontend ships in
  its release's `.mpk` and works with that release's backend only, and releases
  may share a WASM, so `bundle` alone does not say which release runs: approving
  a release approves its frontend, and moving to a release with the same WASM is
  an upgrade like any other (same authority, consent and record; an org run
  writes `UpgradeApp` without running anything on the app's group, and notes a
  refusal as failed). `Node::release_of_app` and `report::release_of_app` read
  it; a record without it (from before, or a raw upload) falls back to the bundle
  (`Node::release_of`, `report::release_of_bundle`). `UpgradeRecord::from_release`
  / `to_release` name both ends of an upgrade. Regressions:
  `a_frontend_only_release_is_an_upgrade_of_its_own`,
  `an_instance_link_opens_the_release_that_instance_runs`,
  `report::tests::a_record_without_its_release_falls_back_to_its_bundle`.
  **Upgrade note (rolling upgrade):** a node on an older release drops the field
  when it applies a `CreateApp` or `UpgradeApp`, so its own copy of the directory
  infers the release from the bundle until it runs this release and the app is
  upgraded again.
  Releases may share an app component and differ in compute, so create and upgrade carry the compute hash of the release they name (`resolve_release_parts`), and apply only checks that a release ships that pair (`SystemState::ships_compute`).
  A create or upgrade by bundle hash alone resolves the compute part of the releases that ship that app component (`SystemState::compute_of`), `400 AMBIGUOUS_COMPUTE` when they differ, and answers `compute: false` when none ships one.
  The app's log carries the pair too: `ComponentCmd::Init` and `Upgrade` name `compute_bundle` beside `bundle`, and apply keeps it in `ComponentHead::compute_bundle`, so a run pins the pair the group runs and never the system record's, which a delayed or failed system write may leave behind.
  A release's parts count against the bundle quota each at its own size (`PackageVersion::part_sizes`; a release from before counts the whole `.mpk` per part).
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
  Two more rules, `--deployments open|granted` and `--deployers
  operator|owner` (`Governance::deployments`, `deployers`; below), are left
  out of the governance JSON when default, so a default cluster attests the
  bytes it did before (`the_default_governance_binds_the_bytes_it_did_before_deployers`);
  the one change after genesis is `SetDeploymentsGranted` (open to granted,
  operator-signed, never back).
- **Deployers** (`SystemState::deployers`, `cluster_owner`, `may_deploy`):
  where deployments are `granted`, an account installs (`InstallPackage`,
  `PutBundle`) only with a live `DeployerGrant` of its own (`account:<id>`) or,
  holding `MANAGE_APPLICATION` org-wide, of its organization (`org:<id>`);
  `until` is judged at the command's own time (`installedAt`, `PutBundle::at`),
  never a node's clock. The API refuses first, before any registry fetch,
  verification or store (`Node::refuse_undeployable` in
  `install_from_registry`, `install_upload` and `POST /bundles`; `403
  DEPLOY_NOT_GRANTED`); `apply` refuses again (`system::NOT_A_DEPLOYER`). So
  does `POST /apps` (and an upgrade run, the consent page) where it would
  install a release from the registry first; an instance of a release the
  caller already sees needs no grant. The operator's installs never need one.
  Grants: `GrantDeployer`/`RevokeDeployer`, the operator's signature
  (`POST /ops/deployers`, `DELETE /ops/deployers/{key}` or `POST
  /ops/deployers/{key}/revoke`; `cluster /
  __grant_deployer / { deployer, until }`, `__revoke_deployer / { deployer }`;
  `at` is not signed: the 428 and the signed retry are two requests) until,
  with `deployers: owner`, the operator names the owner once
  (`SetClusterOwner`, `POST /ops/cluster-owner`, `__cluster_owner`); from then
  on only the owner account or the owner org's org-wide `MANAGE_APPLICATION`
  holders, with passkey consent (`POST /deployers`, `DELETE
  /deployers/{key}`), and the operator's signature is refused
  (`SystemState::deployers_owner`, `authorize`). Revoking keeps what was
  installed and running. `GET /deployers` for the operator and the owner side.
  Regressions: `only_granted_accounts_install_on_a_granted_cluster`,
  `an_open_cluster_installs_as_before_until_the_operator_grants_deployments`,
  `the_cluster_owner_grants_deployers_once_the_operator_names_it`
  (`tests/deployers.rs`), `on_a_granted_cluster_only_deployers_install`,
  `deployer_grants_carry_the_operators_signature_then_the_owners_passkey`,
  `deployments_are_granted_one_way` (`system::tests`).
- **Deployments** (`SystemState::deployments`, `system::Deployment`,
  `Offer`): an owner's (`Deployer`: account or org) list of offered apps, app
  name -> one installed `package@version`. Managed by whoever `acts_for` the
  owner (the account; org-wide `MANAGE_APPLICATION`), each change with that
  actor's passkey consent judged at apply: `CreateDeployment` (`POST
  /deployments { name, owner }`, `cluster / __create_deployment / { name,
  owner }`; the id is `system::deployment_id` of the consent's nonce),
  `OfferApp` (`POST /deployments/{id}/apps`, `deployment:{id} / __offer_app /
  { app, package, version }`; the release installed and seen by the actor;
  an entry keeps its package), `WithdrawApp` (`DELETE
  /deployments/{id}/apps/{app}`; refused while an instance from it runs that
  package; the release stays installed), `DeleteDeployment` (`DELETE
  /deployments/{id}`; nothing offered, no instance from it). Create and offer
  need `may_deploy` for the actor where deployments are granted. Bounds:
  `MAX_DEPLOYMENTS_PER_OWNER`, `MAX_DEPLOYMENTS`, `MAX_APPS_PER_DEPLOYMENT`;
  names `deployment_name_fits`, app names and ids `deployment_segment_fits`.
  `POST /apps { deployment, package, version? }` creates from the offer
  (`api::offered_bundle`, no install, no grant, no `sees_release`) and records
  `AppRecord::deployment`; `CreateApp` checks the offer again
  (`offers_bundle`). `UninstallPackage` refuses an offered release (`offered`);
  `DeleteOrg` refuses an org that owns a deployment. Reads: `GET /deployments`
  (what the caller manages), `GET /deployments/{id}` (public, on fresh state:
  name, owner kind and name, offers), `GET /deployments/{id}/orgs` (managers, `sys_fresh`:
  customer orgs' id, name, instance count, apps and versions, first created).
  Not done: the spec's migration of old installs into deployments, transfer,
  the lockout guard, the consent page naming the deployment. Regressions:
  `tests/deployments.rs`, `deployments_are_their_managers_and_keep_what_they_offer`,
  `deployment_changes_carry_their_managers_consent`,
  `state_from_before_deployments_reads_with_none_and_writes_as_before`,
  `a_deployment_deleted_through_the_leader_is_gone_on_a_follower_behind`.
- **Sign-up policy** (`Governance::sign_up`, `--sign-up open|invite|closed`,
  left out of the governance JSON when `open`): `CreateAccount { invite }`
  is judged at apply by `SystemState::admits` (`open`: anyone; `invite`: an
  org invite, which joins the org, or a cluster sign-up invite; `closed`: a
  sign-up invite only; an invite presented is checked on every policy).
  The invite is `SignUpRedemption { inviteKey, now, signature }`, the token
  key's `proof::sign_invite_redeem` over the new account, spent in
  `used_proofs`; refusals start with `system::SIGN_UP_PREFIX` (`403
  SIGN_UP_REFUSED`), and `register/begin` and `register/finish` refuse first
  (`api::refuse_sign_up`, fresh state before refusing; the finish before its
  challenge is spent). `SystemState::sign_up_invites` (`CreateSignUpInvite`,
  `RevokeSignUpInvite`, bounded by `MAX_SIGN_UP_INVITES` and
  `MAX_SIGN_UP_INVITE_USES`) and `SetSignUp` are run by the operator's
  signature until the cluster's owner is named, then the owner's passkey
  (`SystemState::runs_sign_up`; `__sign_up`, `__create_sign_up_invite`,
  `__revoke_sign_up_invite`). Routes: `GET /sign-up` (public), `/ops/sign-up[/invites]`,
  `/sign-up[/invites]` (owner). Regressions: `tests/sign_up.rs`,
  `system::tests::sign_up`.
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
  `call_local`'s runs (calls, follow-ups, timers; not a read-only member's) and to a
  component's actions (`NodeHttp`, "Component executor"), for
  a bundle that is a signed release declaring prefixes; identity = (package,
  pinned signer), approval = `Org/Account::outbound_approvals[signer]`, set by
  the account, or for an org whoever holds `MANAGE_APPLICATION` org-wide, with
  `SetOutboundApproval` (`POST /orgs/{org}/outbound`,
  `/auth/outbound`, passkey consent; `merge_approval` at apply). The request is
  made inside the propose lock, on its own thread (`run_blocking`); a leader-race
  re-run sends it again. A component's calls, timers and deliveries get the same gate
  (D76, "Component executor").
- **Files** (`files.rs`, `files/seal.rs`, `files/wrap.rs`): a file is 64 KiB AES-256-GCM STREAM chunks
  (`FileSealer`, `FileOpener`) under a random data key.
  The nonce is 4 zero bytes then the big-endian chunk index; the AAD is `calimero.file.v1` then a last-chunk flag byte.
  `FileOpener::finish_expecting` checks the plaintext size and SHA-256 against the mark, as GCM does not commit to its key.
  A file is stored as segments: `SegmentWriter` cuts the ciphertext stream into segments of `SEGMENT_CHUNKS` (256) whole chunks (`SEGMENT_BYTES`, 16,781,312 stored bytes; the last segment is the rest), and `SegmentReader` joins them, checking each against the list.
  A segment's hash is the SHA-256 of its bytes and the file id is `file_id`: SHA-256 of `"calimero.file.segments.v1\0" || u32 count || h0 || ... || hn-1`, so an id never equals a plain blob's content hash.
  A mark (`FileMark`) and a record (`FileSeal`) list the segments (`segments`, required, hex, at least one); `FileMark::segment_hashes` is the one check that the list is well formed and hashes to the mark's id.
  `wrap_key` wraps the data key as salt || nonce || AES-256-GCM under a subkey of `backup::file_key` (per app, from K_c, not the TEE key), with the app id as AAD.
  `wrap_for_job` re-wraps a secret to a job node: ephemeral X25519, then HKDF over both public keys, the kind (file key or noise seed), the length-prefixed app and run ids, attempt and index.
  The job node opens it through `SealedTransport::derive`, which never returns the raw DH output and labels its HKDF (`calimero_sealed::derive_shared`, shared with the wrap side).
  `noise_seed` and `run_token` derive a run's seed and capability from the file key.
  **Exporting a file** (`api/exports.rs`; tests `component_files.rs`, `pipeline_runs.rs`): `POST /apps/{app}/files/{id}/export { recipient }` (64 hex, an X25519 public key) and `GET /apps/{app}/files/{id}/segments/{k}` take the `/files` gate (`files_gate`: `api::may_handle_files`, fresh system read, even on a public app) and judge the file on the app's leader (`Node::file_record_strong`: `internal/blob/held` with `record`, which answers the size and `FileSeal`), else `404`; a segment past the last is `404 NO_SUCH_SEGMENT`, and one is read from this node's store or a replica (`segment_from`).
  The export answer is `{ app, id, size, sha256, segments, ephemeral, wrapped }`; `wrapped` is `nonce || AES-256-GCM` of the data key (unwrapped from the `FileSeal` with the app file key, then zeroized) under `HKDF(X25519(ephemeral secret, recipient))`, salt `calimero.export-wrap.v1`, info `ephemeral public || recipient public || u32 length-prefixed app id || u32 length-prefixed hex file id` (`calimero_sealed::wrap::wrap_export`, over `derive_shared`, which refuses a low-order key: `400 BAD_RECIPIENT`, as does a recipient that is not 64 hex).
  The construction lives in `calimero-sealed` (`wrap.rs`, with the AES-GCM helpers `wrap_for_job` shares, and `file.rs`, the file STREAM format that used to be `files/seal.rs`) so `calimero_client::Client::export_file` opens it with `unwrap_export`, checks the segment list against the file id, each segment against its hash, and the size and plaintext SHA-256 with `finish_expecting`, and returns bytes only when all match.
  An account may export `MAX_EXPORTS_PER_MINUTE` (30) times a minute per node (`ExportLimits`, an in-memory token bucket over at most `MAX_TRACKED_ACCOUNTS` accounts): `429 TOO_MANY_EXPORTS`.
- **Receipts**: every `/intents` and `/views` answer carries a `Receipt` signed by the
  answering node's receipt key (Ed25519, made on the node's first start and kept
  in `<data_dir>/signing-key` wrapped under K_r, `signing_key.rs`: a restart
  keeps it, only the release whose mero-kms gives that K_r opens it), over
  `(app, index, SHA-256(JCS(result)), node)`. The key is attested on its own
  (`GET /attestation?purpose=receipts`, report data `nonce || receipt_key_binding`),
  and `GET /apps/{app}/attestation` gathers every replica's
  (`Client::verify_app_cluster`).
- **Account proofs** (`api/account_proof.rs`, `Node::account_proof`): `POST
  /auth/account-proof { nonce, audience }` (a passkey session, `holder`: never
  an API key or a package-bound session) answers
  `calimero_client::attest::AccountProof`, the receipt key's signature over
  `"calimero.account-proof.v1\0" || JCS({ cluster, node, account, nonce,
  audience, issuedAt, expiresAt })`, valid `ACCOUNT_PROOF_TTL` (5 min); limits
  `ACCOUNT_PROOF_NONCE`, `MAX_ACCOUNT_PROOF_AUDIENCE`, plain text only.
  `GET /ops/nodes/{id}/receipt-key` (ops token) answers the key
  `Node::session_issuer` holds for that node, so the operator (mdma) checks a
  proof from any node. Regression: `tests/account_proofs.rs`; vector:
  `an_account_proof_binds_its_statement_and_key`.
- **KMS evidence**: `GET /attestation?purpose=kms` adds `kms` (`config.kms_keys`: the
  KMS quote merod accepted for its storage key, and the boot script's record for the
  data disk's, `--kms-disk-evidence`) and binds it (`kms_report_data`). Without a
  record, or without `purpose=kms`, the answer is the plain transport form.
- **Release attestations** (`attest.rs`, `release_attestation`, `serve_release`): a
  quote over a manager's nonce and `ReleaseClaim`, for `merod attest` before the node
  has any key. Not a route of a running node: `serve_release` is its own tiny server
  (that one route, `MAX_CONCURRENT_RELEASE_QUOTES`), run only by `merod attest --serve`.
- **Access is judged fresh**: every request carrying a credential, every gated request, an anonymous
  `GET /apps/{app}` (#4723) and every client call's leader read the system state through `sys_fresh`
  (a read index), so a removal committed elsewhere holds before the next request. Regression:
  `an_app_made_private_is_hidden_from_anonymous_callers_on_a_follower_behind`.
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
- **Removing nodes**: `DELETE /ops/nodes/{id}` (system leader; `421 NOT_LEADER` elsewhere) writes `RemoveNode` (refused while an app keeps a replica there) and changes the system group's voters, promoting a registered learner while fewer than 3 are left; `SystemState::removed_nodes` records it; only the operator's signed `/ops/nodes` registers it again. Regression: `a_node_is_removed_once_no_app_keeps_a_replica_on_it`.
- **Upgrade keys** (`ApiKeyRecord::upgrades`, `types::UpgradeScope`; docs: protocol
  "Upgrade keys", cloud "Deploy from CI"): `POST /auth/apikeys { upgrades: { app,
  package? } }` (passkey consent; `api_key_args` states `upgrades` only when set, so
  other keys' consents read as before) makes a key bound to one app, its package and
  that package's pinned signer, plus the public half of an Ed25519 key derived from the
  key (`proof::api_key_public`). `api::principal` refuses it everywhere (`403
  UPGRADE_KEY_ONLY`); only `app_upgrade` takes it (`api::credential`), for `{ version }`
  of its package (`SystemState::key_may_upgrade`), signing the statement in place of
  the step-up (`proof::sign_key_upgrade`, carried in `UpgradeApp::consent`). `authorize`
  verifies it under the key the log holds then (revoked or expired: refused) and spends
  it; every other upgrade rule is unchanged. `UpgradeRecord::api_key` names the key
  (`lastUpgrade.apiKey`). Regressions: `tests/upgrade_keys.rs`.
- **Job nodes** (`NodeRole::Job`, `job_node.rs`, `node/job.rs`): `POST /ops/nodes { id, addr, zone?, role?, pool? }` (operator-signed; `role` is in the signed arguments unless it is `replica`, `pool` only when set; `"replica"` default, `"job"`; any other role is refused) registers a node that only runs compute. It is in the directory but never a Raft member (not even a learner: a member keeps its log sealed under K_c, which a job node never holds), never a replica, blob (a sealed file included, `a_sealed_file_is_never_sent_to_a_job_node`) or bundle target, majority member, presence peer, CSP origin or cluster-key recipient. Every site that picks nodes for the cluster's own work reads `SystemState::replica_nodes()` / `replica_node(id)`, never `nodes`; `/ops/apps/{app}/replicas` naming a job node answers `400 JOB_NODE`, `internal/cluster-key` for one is refused, `/ops/placement` lists it with `role` and app counts stay replica-only. `DELETE /ops/nodes/{id}` removes one without touching system voters. A node's role is fixed (`RegisterNode` refuses another role for an id; remove it first) and `/ops/nodes` refuses `role: job` for a system voter, learner or `--peers` id (`NODE_HOLDS_CLUSTER`); removal changes system membership only for a node that is in it. Nodes are reached by id: Raft's `PeerTransport::send` and `internal/cluster-key` take the address `Node::replica_addr(id)` holds (the directory's, else `--peers`; none for a job node), and address-only callers (`Node::peer`, so `peer_post` and streams) resolve it by a normalised comparison (`system::normalize_addr`) and refuse anything that is not a replica's, so the cluster secret never leaves for a job node. `RegisterNode` refuses an address another id holds (`ADDRESS_TAKEN` at the API). A ceremony holder is resolved through `replica_node`. Registering a job node (`POST /ops/nodes`, `role: job`) answers `409 CLUSTER_NOT_UPGRADED` unless every replica node, voter and learner runs at least this node's release, so no job record exists while an older node could misread it (upgrade the voters first). Upgrade the voters before anything else: the `RegisterNode` refusals (role fixed, address taken) bind only nodes that run this release. A node registered as a replica is admitted as a learner only after it attests its transport key as a cluster node (`attests_as_replica`): a job node registered as a replica never is, and `--peers` must list only replica nodes. `merod run --job-node` calls `run_job(JobConfig)`: no store, K_c, KMS, Raft or system group; it serves `/health`, `/attestation` (only `purpose=jobs`, answering `jobKey` bound with the transport key, release, cluster and node id in one quote; every other purpose or none is 404, so no channel that attests a transport key opens to it; a cluster node answers `purpose=jobs` with 404) and `GET /job/load` (`{ running, capacity }`, capacity is `--job-slots`, default `RUNS_PER_JOB_NODE` = 2). Its Ed25519 job key is made in memory at start. Leader side: `Node::attest_job(id, app)` fetches `purpose=jobs` over a fresh nonce, verifies it with `verify_job_key` under the peer policy (`--peer-profiles`, `--peer-min-release`), keeps the result `JOB_ATTEST_TTL` (60 s) for as long as the directory's `node_epoch` for the id is unchanged (any registration or removal) and until a channel to it fails (`forget_job`); it fails outright when the directory could not be brought up to date. `attest_job` also refuses a release the app's org or account has not approved (`release_may_hold`, as `may_send` does), and `/ops/placement` reads a job node's release through the same cached attestation; `Node::job_load(peer)` asks over a sealed channel pinned to the attested transport key. Regressions: `tests/job_nodes.rs`, `placement::tests::a_job_node_is_never_placed`.
- **Job runs** (`job_node/{admission,bundles,runs,exec}.rs`, `job_wire.rs`; tests `job_runs.rs`): a job node runs one attempt of a compute run per assignment, all inside the sealed channel. **Who may give it work**: only the cluster whose identity key it was started with (`merod run --job-node --cluster-identity HEX`, from `GET /ops/cluster` / `meroctl ops cluster`). The identity is an Ed25519 key derived from K_c (`backup::cluster_identity_key`, seed `HMAC-SHA256(K_c, "calimero.cluster-identity.v1")`), so a valid signature shows a node of this cluster signed; the job node fetches and attests nothing about the sender, and has no `--peers`; a key that is not a curve point or is of small order is refused at start (`check_cluster_identity`), signatures are checked strictly, and the identity is bound into the job node's `purpose=jobs` quote (`AttestationDoc::cluster_identity`, `job_key_binding`) so the leader's `attest_job` refuses a job node that serves another cluster. `PUT /job/bundles/{hash}` takes a component only with `x-bundle-signature`, the identity's signature over `"calimero.compute.bundle.v1\0" || hash` (`403 BAD_SIGNATURE`), then checks the hash and prepares it once on the compute engine (`MAX_JOB_BUNDLES` kept, least recently used first); only a compute component (`calimero:app/compute`, its ABI listing compute methods) prepares, and an app component is refused with `400 BAD_BUNDLE` even when the cluster signed it, so the job node never links storage, state or the network for it. `POST /job/runs` (at most `MAX_ASSIGNMENT_BYTES`) takes an `Assignment` (`job_wire`: app, run, attempt, method, args, bundle, caller, owner, `startTsMs`, budget, inputs `{id = the file id, size, sha256, segments = the hex SHA-256 of each segment, wrapped}`, wrapped seed, hex token, `signature`) signed by the identity over a length-prefixed binary message of every field (`assignment_message`, big-endian integers; `Node::sign_assignment` is the cluster side). Refusals, in order: shape (`400 BAD_ASSIGNMENT`/`BAD_BUDGET`, duplicate input ids, `413 TOO_MANY_INPUTS`/`INPUT_TOO_LARGE`/`INPUTS_TOO_LARGE`), `403 BAD_SIGNATURE`, then an assignment already held answers its state (`409 RUN_CONFLICT` if it differs), `503 BUSY` at the node's `--job-slots` runs waiting or running or when the finished outputs held on disk plus the most each active run and the new one may still write exceed `MAX_FINISHED_BYTES` (nothing is evicted, the leader collects or deletes), `409 NEED_BUNDLE`, `400 NOT_COMPUTE` (the compute ABI must list the method as a compute method), `400 BAD_WRAP` (every wrapped key opens at admission through `unwrap_for_job`, so a key made for another node, run, attempt or index never gets in). Runs are keyed by `(app, run, attempt)` (run ids are per app) and every other route is `/job/runs/{app}/{run}/{attempt}/...` with the app id URL-encoded; they need the run token in `x-run-token` (constant-time compare); a wrong token answers exactly as an unknown run, `404 UNKNOWN_RUN`, which the leader treats as lost. `PUT .../inputs/{i}/{k}` takes segment `k` of input `i`, in any order, only if its length is the segment's and its SHA-256 is the one the assignment lists (`400 BAD_SEGMENT`, `404 NO_SUCH_SEGMENT`); an input whose list does not hash to its id is `400 BAD_ASSIGNMENT`; each segment is written at `k * SEGMENT_BYTES` in the input's scratch file, and when the last one is in the file is streamed through the opener from disk (`exec::input_matches`) to prove it opens under the run's key to the declared size and plaintext SHA-256; an input that does not match ends the run on the spot with a signed `integrity` report, since the leader sent it; the leader sends the segments of an input one at a time, from its store or fetched from a replica; the run starts on its own OS thread when every input is in (at once for none). Sealed request bodies are decrypted and buffered before any route check, bounded by the sealed in-flight cap; that is accepted. `GET .../{attempt}` answers `{ state: awaiting|running|done|lost, progress?, report?, signature? }`: `done` carries the signed report, a failed run included; `lost` is a run this node could not serve (`Unavailable` trap, thread panic, node file I/O, an input that fails to open during the run, which is the node's own disk), never signed, which the leader restarts elsewhere with the next attempt. `GET .../outputs/{i}/{k}` gives one segment of an output's ciphertext, read by range from the scratch file (`404 NO_SUCH_SEGMENT` past the last), and `.../outputs/{i}/key` its data key (hex), once done; outputs are listed in the order the run finished them. `DELETE` sets the cancel flag, waits `CANCEL_WAIT` for the run to stop, answers its last status and drops it with its scratch files (a run that did not stop stays and answers `running`); runs not deleted are swept `JOB_RESULT_TTL` after they end, runs still waiting for inputs end lost and are swept after `AWAITING_INPUTS_TTL`; the scratch directory is emptied at start, so a restarted node knows no runs. The run is the compute component's `run` export, invoked as an action with a compute context over the files of the assignment only (`Assignment.bundle` names the compute component's hash): inputs are streamed from the scratch ciphertext through `FileOpener` and checked with `finish_expecting` at the end, and a failure then is a read error, so the run is lost, never signed as `integrity`. Outputs are sealed to disk under a fresh key each (`NewFile.ciphertext` stays empty); engine `WasmtimeEngine::for_compute`, run through `WasmtimeEngine::run_compute` (`ComputeRun`), the setup `tools/mero-compute` also calls, so only the file provider differs. A trap maps to the reason structurally (`calimero_app_engine_wasmtime::ending_of`, which `exec::classify` maps onto `FailReason`): fuel, memory, cancelled, trap (fault, stack, any other denied call), app-error; a wall clock that ran out (`Timeout`) is a lost attempt, so the outcome never depends on the machine and only fuel is the deterministic cap. A trap or app-error report also carries the text (`Report::message`, signed, at most `MAX_REPORT_MESSAGE_BYTES`). The `Report` (`job_wire`) holds app, run, attempt, status, reason, the result bytes, outputs `{id = the file id, size, sha256, segments}` and usage as integers (`cpuMicros` thread CPU, `gibMicros` peak memory times wall, `fuel`); the job key signs `"calimero.compute.report.v1\0"`, the SHA-256 of the assignment message it ran (so a report stands for that assignment only) and the length-prefixed fields, output ids, hashes and segment hashes as their 32 decoded bytes (`report_message`); one encoding only: ids and hashes are lowercase 64 hex, a done report has no reason, a failed one has a reason and neither result nor outputs, and `verify_report(job_key, assignment_digest, report, signature)` is the one check the leader uses. `Assignment`'s `Debug` hides the token and the wrapped keys; the seed and keys live in `Zeroizing`, except the copy of the seed the host core keeps in its `Invocation` for the run's life. A job node feeds no metrics series: a compute run leaves every `calimero_app_*` series of the cluster unchanged until the leader counts it. The leader's reads from a job node (`attest_job`, `job_load`) stop at `MAX_JOB_RESPONSE_BYTES`.
- **Compute runs** (`component/runs.rs`; tests `component/tests/runs.rs`, `system/queue_tests.rs`, `component_compute.rs`):
  A call, timer, `init` or result method starts a run with `runs.start`.
  The host puts `Outcome::compute_starts` (and `compute_cancels`) in its outcome, and the node fills the app's `owner` (`Outcome::name_owner`).
  The payer is the leader's to read from fresh system state when it acquires.
  A read-only member's call that starts or cancels a run is refused like one that writes keys.
  `apply` (status `Ok` only) writes a `RunRecord` per run as a hidden row (`rows::CRUN`, by run id) and adds its id to `ComponentHead::active_runs`.
  At most `MAX_ACTIVE_RUNS` runs are active; a start past it or of a known id is dropped.
  `finished_runs` keeps the last `FINISHED_RUNS_KEPT` (256) finished ids, oldest first, and a record leaves with its id.
  The record pins the bundle that ran at start, so an upgrade does not move it.
  It pins the compute component the same way: `RunRecord::compute_bundle` is the group's own (`ComponentHead::compute_bundle`, set with the bundle by the app log's `Init` or `Upgrade` entry) at the moment the start applies, never a value the leader read from the system group.
  Apply never reads system state, because the system group and the app's log are different logs: a delayed or failed system write cannot give a run a compute component that did not ship with the group's bundle.
  An upgrade that would leave an active run's result method undeclared (`RunNeedsResultMethod`, checked on the leader inside the proposal) is refused.
  An app whose running release has no compute component (a raw upload, or a release without one) cannot start a run: `runs.start` answers the soft error `NO_COMPUTE_COMPONENT` and nothing is recorded.
  A run recorded before this has no `compute_bundle`; the leader that drives runs must treat none as no compute component.
  An input of a run is one of the app's own sealed files (a blob record with a `file` seal), named once; the host refuses a plain blob, an unknown id and a file the same run just made.
  A start whose input is no longer a sealed file the app holds when it applies is dropped.
  An active run's inputs count as references of the app (`pin_inputs`: one more at the start, one less at the final), so R8 keeps them even when no state refers to them.
  They are forgotten `BLOB_IDLE_MS` after the run is final, like any file nothing refers to.
  Two more entries move a run.
  `computeAttempt { run, attempt, node, jobKey, assignmentDigest, tsMs }` applies to an active run whose cancel was not requested, when `attempt` is the next number and at most `MAX_ATTEMPTS` (3) and both keys are hashes (64 lowercase hex); it sets `running`.
  `computeFinal { run, attempt, outcome, report?, signature?, outputs, usage, result, baseIndex, tsMs, idTerm, idSeq }` ends it.
  The first valid final of the run's latest attempt (0 before any) wins and any later one is a no-op.
  A job outcome (`done`, or `failed` with `fuel | memory | trap | app-error | integrity | cancelled`) needs a report and signature that `verify_report` accepts under the latest attempt's `jobKey` and `assignmentDigest`, for this app, run and attempt, with the same status and reason, and `outputs` and `usage` exactly the report's.
  A `cancelled` final (or `failed cancelled`) needs `cancelRequested`, and is either the leader's (no report, zero usage) or carries the job node's report (status failed, reason cancelled) that verifies like any other, with its usage.
  Other leader outcomes carry no report, no outputs and zero usage: `failed lost` needs every attempt used, `failed quota` (the outputs would pass the payer's file quota) no further state, `failed timeout` a run that was assigned, `failed refused` (an acquire refused for good) a run with no attempt.
  Outputs are bounded (`MAX_NEW_FILES`, `MAX_FILE_BYTES` each, `MAX_COMPUTE_FILE_BYTES` together), and a job's usage cannot pass its caps (fuel; CPU micros and memory-GiB-micros at most the wall cap plus `DEADLINE_GRACE`, 60 s).
  An output id that is not a blob id, is already a blob of the app or is listed twice is refused.
  A final registers each output as a sealed file of the app in the same entry and before `result` applies (`BlobTable::add_file`, the path a run's own files take, with the plaintext hash and wrapped key), so a `BlobRef` the result method stores is counted.
  An output the result method does not store is forgotten `BLOB_IDLE_MS` later, and the org's file quota is released when the leader collects it.
  `AppMachine::blob_view_with_outputs` gives the leader's result run the records the final will leave, so `storage_put` accepts those references.
  A final marks each output `FileSeal.output`, the flag that makes it publishable (only a signed done report carries outputs).
  The result method takes one argument shaped like the SDK's `ComputeOutcome`: camelCase `run, method, attempt, status, reason, result, outputs: [{ id, size, sha256 }], usage`, where an output `id` is the 64-hex blob id (a `BlobRef`) and 64-bit integers are decimal strings.
  The compute fixture's `onDone` takes that shape (`compute_fixture.rs` in `calimero-app-engine-wasmtime`).
  The cluster tests' fixture (`apps/v2-fixtures/compute`) takes it as the SDK's `ComputeOutcome` and stores each output's `BlobRef` when the run is done.
  Every permanent refusal changes nothing and answers `AppError` with the reason as `error`; only a stale `baseIndex` answers `conflict` (retry on the current state) and leaves the run active.
  `init` starts runs on its own bundle.
  The result method runs with `runs_provider(Some(run))`, which leaves out the run that is ending, so its own start of the same job is a new run.
  `new_file_bytes` counts only the new files of a final's result method for the one charge gate, and `None` on overflow refuses the entry: the leader charged and stored the outputs before it took the proposal lock.
  A final over the log's cap loses only its result method's outcome, and its outputs stay registered.
  A final over the cap even without it is not built (`FINAL_TOO_LARGE`), and the leader ends the run `Failed{tooLarge}`, its own reason, not a timeout.
  A run record that cannot be read makes the host deny start and cancel (`RUN_RECORDS_UNREADABLE`).
  A run being cancelled is not the answer to a start of the same job.
  Applying a final records it, releases the inputs, registers the outputs, then applies `result` as a call's outcome (only if its status is `Ok`; its own starts and cancels included).
  A queued run whose fuel exceeds a lowered `computeFuelPerOrg` is marked `RunSlot::refused`, and the leader ends it `failed refused`.
  The system group queues runs (`SystemState::compute`, `RunSlot`).
  `AcquireRun` (idempotent; payer must be the app's, fuel at most `MAX_COMPUTE_FUEL`) and `ReleaseRun` run the grant pass.
  Capacity is per pool: `SystemState::pools` (`POST /ops/pools`, operator-signed `cluster / __set_pools / { pools }`, refused `409 UPGRADE_PENDING` by `Node::require_current_release` while a node is behind) names each pool's `runsPerNode` and `maxMemoryMib`, a job node's `NodeRecord::pool` (empty is `DEFAULT_POOL`) puts it in one, and a pool's capacity is its nodes times `runsPerNode` (`pool_capacity`); a node in a pool the cluster does not define adds none.
  A run goes to the smallest pool whose `maxMemoryMib` fits its caps (`pool_for`, `RunSlot::pool`), `AcquireRun` refuses one that fits none (`RUN_NO_POOL`, permanent), and `pick_job` keeps only job nodes of the slot's pool; `AcquireRun { pool: Some(name) }` (a pipeline step's) takes that pool instead when it fits the memory cap.
  `SetPools` refuses removing a pool that still has job nodes or runs (`POOL_IN_USE`).
  `SystemState::price_table` (`GET`/`POST /ops/prices`, operator-signed `cluster / __set_price_table / { table }`, same release gate) holds the `calimero_pipeline::PriceTable`; `SetPriceTable` accepts only `version` = current + 1 (`PRICE_VERSION`) and a nonzero fuel rate (`BAD_PRICES`).
  With no pools it is the legacy single pool: registered job nodes times `RUNS_PER_JOB_NODE` (2).
  Whether a node is reachable or runs a release an org approved is the leader's attested view, not replicated state, so `apply` cannot count it and the leader bounds a run's wait instead.
  A payer's limits (`SystemState::payer_quotas`: runs, fuel, file bytes) are its override in `SystemState::org_quotas` where it has one (`POST /ops/quotas` with `org`: a field set to `null` clears it; operator-signed `cluster / __set_org_quotas / { payer, quotas }`, refused `409 UPGRADE_PENDING` while a node is behind; all fields cleared removes the entry), else the cluster's.
  `grant_runs`, the `AcquireRun` fuel refusal, the refused marking and the file-bytes charges read them; lowering an override never revokes a granted run, new grants wait.
  Free places go to the payers in turn: the one served longest ago (`run_served`, a count of grants, kept only for payers that still have runs), then the one whose oldest waiting run has the smallest `seq`; and a payer at `maxConcurrentRunsPerOrg` (4) or over `computeFuelPerOrg` (10 x `MAX_COMPUTE_FUEL`), or its override, is passed over for the pass.
  `RegisterNode`, `RemoveNode`, `SetQuotas`, `SetPools`, `TransferApp` (a personal app's payer moves) and `remove_app` (`DeleteApp`, `DeleteOrg`, `DeleteTeam`) re-run it, and a granted run is never taken back.
  `AcquireRun` answers `{ state: granted | queued | refused, code, retryable }`.
  Retryable codes are `NOT_PAYER` and `APP_QUEUE_FULL`; permanent ones are `NO_SUCH_APP`, `FUEL_OVER_RUN_MAX`, `FUEL_OVER_QUOTA`, `RUN_NO_POOL` and `RELEASED`.
  It refuses a run id released before (`released_runs`, the last 256 per app), past `MAX_ACTIVE_RUNS` runs queued for the app, and a run whose fuel alone exceeds `computeFuelPerOrg`.
  `AcquireRun` and `ReleaseRun` are proposed only once a job node is registered, which already requires every system node to be on this release.
  `computeStarts` and `computeCancels` follow the rule files do: a node never moves to a new release in place (its storage key is per release), so an older replica that would miss run records is retired with its release, and an older node cannot prepare a bundle with compute methods (unknown kind, forbidden import).
- **Driving compute runs** (`node/component/compute.rs`, `compute/{job_io,finish}.rs`; tests `compute_runs.rs`):
  `lib.rs` calls `Node::drive_runs` every `JOB_POLL` (2 s) and when a call commits (`runs_kick`).
  For each component app this node leads, one task per app frees stray slots and starts a step task for each active run not already being stepped.
  One step runs per run at a time, so a run stuck in a long transfer never delays another run's step or cancel.
  A step that finds this node no longer leads its term does nothing.
  A step works from the replicated record and the system state, so a new leader resumes where the last stopped.
  What the leader keeps in memory (`Watch`, per run and leadership term) is one healthy-time accumulator, when a job node last answered in that time, the valid report a job node gave, whether this leader sent the attempt, the last progress and the outputs staged for a final.
  The healthy-time accumulator counts the time in rounds whose system read succeeded; the lost clock, the run deadline and `QUEUED_RUN_TIMEOUT` count only it, so a system-group outage burns none of them.
  A new leader starts the lost clock at takeover.
  The held report is kept per attempt, so the leader's own failing charge, push or propose never counts as silence; a job node that will not hand its outputs over still does.
  Outage credit is capped at `QUEUED_RUN_TIMEOUT`, and the lost and cancel clocks also end at 10 times their limit in wall time.
  A running run ends by timeout at an absolute ceiling of wall cap + `DEADLINE_GRACE` + `QUEUED_RUN_TIMEOUT` on the app clock from the attempt's `tsMs`, which survives leader changes.
  The unplaced and cancel clocks start when the current leader first sees the run in that state and restart with each term: unplaced `QUEUED_RUN_TIMEOUT` of healthy time, cancel `CANCEL_DEADLINE` (20 s) of healthy time.
  A loss the job node announces (`lost`, a new key, `UNKNOWN_RUN` after sending) first finishes from a held report (`lost_unless_held`).
  A queued run takes `AcquireRun` first (only when a job node is registered; the payer comes from fresh system state; a permanent refusal ends the run `Failed{refused}`, a retryable one waits).
  Once granted, `Node::survey_jobs` attests each job node and asks its load outside the assignment lock, then `pick_job` under it picks, among the nodes of the run's pool, the fewest runs, then the lowest id; a node that lost an attempt is picked only if no other is free.
  The assignment is built and delivered (component, assignment, inputs) to that node first, and only then is the attempt committed with the job key and the digest, so a busy or unreachable node (503 is not a failed channel) spends no attempt and the next candidate is tried.
  A delivered run whose attempt did not commit is deleted.
  Choosing and reserving a place is serialized (`assigning`, `reserved`), the transfers are not.
  A granted run that no job node takes for `QUEUED_RUN_TIMEOUT` (10 min) ends `Failed{refused}`, and one the queue grants no place in that time (no job node, or every place held) ends `Failed{notGranted}`; `NotGranted` and `Refused` need a run with no attempt.
  A run whose pinned compute component is no longer installed (a fresh system read confirms it) ends `Failed{refused}` at once.
  A wall clock that ran out on a job node is a lost attempt, so such a run is retried and ends `Failed{lost}` after `MAX_ATTEMPTS`.
  A new term keeps the outputs staged for the same attempt (`ComputeState::take`) so they are charged once, and a charge that failed is refunded only if it may have committed (`charge_may_have_landed`).
  The run's record and the `GET /apps/{app}/runs/{run}` answer show progress and the failure text (`final.message`, the job's signed `Report::message`) only to those who may list the app's files.
  The leader assigns and pushes only the run's compute component (`RunRecord.compute_bundle`, named by `Assignment.bundle`, fetched by `push_bundle` through `bundle_wasm` and signed with `sign_bundle`), never the app component.
  A queued run recorded without one (only a record from before compute components existed) ends `Failed{refused}` at once, before a slot is taken or a job node contacted.
  Usage is billed to the run's `bundle`, the app component that started it, and the result method runs on the replicas' app component.
  The assignment is a pure function of the record, the app's blob records (each input's `FileSeal` wrapped key) and the job node's keys (`build_assignment`).
  The assignment is signed by the cluster identity, and `wrap_for_job_derived` derives each wrap's ephemeral key and nonce from the app file key, so a new leader rebuilds the very bytes the digest names; a digest or job key that differs means the attempt is lost.
  The run token is `HMAC(file key, run | attempt)`.
  A run polls `GET /job/runs/{app}/{run}/{attempt}`, and `running` is the heartbeat.
  `404 UNKNOWN_RUN` or `awaiting` makes the leader send (component on `NEED_BUNDLE`, assignment, inputs) unless it already sent in this leadership, in which case the attempt is lost.
  No answer for `JOB_LOST_AFTER` (20 s) loses the attempt, after the leader looks at the record, a held report and one fresh poll (`rescue`).
  A `lost` answer, or a report that does not hold up, loses the attempt too.
  A report holds up when its app, run, attempt, signature (under the recorded job key and assignment digest) and usage (within the caps) are right, and a cancel was asked for if it says cancelled.
  The next attempt goes to another job node, and at `MAX_ATTEMPTS` the run ends `Failed{lost}`.
  A done report is checked, then its outputs are charged to the payer before any is fetched (`QUOTA_EXCEEDED` ends the run `Failed{quota}`).
  The outputs are fetched one segment at a time: each segment is checked against the signed list as it arrives (a mismatch loses the attempt), fed to a running opener and stored and pushed before the next is asked for; the size and plaintext hash are proved after the last (`finish::store_output`).
  Each answer counts as the job node being alive, and the number of segments of an output already stored is kept in the watch (`Watch::fetched`), so a cut fetch resumes at the next segment, reading the stored ones back from this node.
  Each output is then stored as a run-created file is: its data key wrapped under the app file key into the `FileMark` (a `FileSeal` once registered) and the ciphertext pushed to a majority of the group's voters through `push_files`.
  The final entry registers them (see **Compute runs**), and the result method runs inside it (`blob_view_with_outputs` lets it store their `BlobRef`s).
  The final is built by `logged_entry` inside `propose_led`; a final answered `conflict` is run again, and one the record shows still active after commit is refused for good (a reported one makes the attempt lost and its staged outputs are discarded).
  Staged outputs are undone (refund, delete bytes) on every exit but the commit of the final that holds them.
  The bytes are deleted only after a barrier and a strong read show the app holds no blob of that id (`delete_unmarked_outputs`), since another leader's final may register the same content, and a replica refuses to delete bytes an app holds.
  A sweep (`sweep_watches`) undoes the staged outputs of ended runs the app holds no blob for, and counts a final whose proposal's answer was lost; a final the state refused for good is not proposed again until the record changes.
  After a final commits the leader releases the slot, deletes the job run and adds the usage to `calimero_app_compute_*` and `calimero_app_calls_total{kind="compute"}`; slots whose run is no longer active are released each round.
  Retention releases the quota of an output the result method did not store, as for any file.
  A cancel first takes a report the job already has (a `done` run keeps its outputs), else deletes the job run and ends the run `Cancelled` with the usage in the job node's signed cancelled report, or with none if it does not answer.
  A run that did not stop is deleted again next round, but `CANCEL_DEADLINE` of healthy time from the first sight of the cancel ends it, from the held report if any, else `Cancelled` with no usage; later reports are refused and the first final wins.
  No final report by the attempt's `tsMs` + `wallSecs` + `DEADLINE_GRACE`, less this leader's outages, ends it `Failed{timeout}`.
  Every job-channel failure goes through `forget_job`.
  `job_call` runs every control request (status poll, delete, load, output key) in its own pool (`MAX_INFLIGHT_JOB_CONTROL`) within `JOB_CONTROL_TIMEOUT` (5 s), and a wait of that long for a place is `BusyHere`: it is this node's queue, not the job node's silence, so the poll is `Deferred` and nothing is forgotten.
  The attestation fetch has `JOB_ATTEST_TIMEOUT` (10 s), so a frozen job node cannot stall a round or starve control calls.
  `job_transfer` (assignment, component, inputs, output bodies) has its own pool (`MAX_INFLIGHT_JOB_OPS`) and 300 s.
  A cancel whose poll gets no answer ends from the held report if any (while that report only waits on the leader's own writes the run stays open, up to the run ceiling), else cancelled with no usage, without a delete.
  `GET /apps/{app}/runs/{run}` and `GET /apps/{app}/runs?limit=&before=` (`AppMachine::runs_page`, newest `startTsMs` first, ties by run id descending, `limit` 1 to 100, a `before` cursor of `startTsMs:run` bounded by `MAX_RUNS_CURSOR_BYTES`, at most `MAX_ACTIVE_RUNS` + `FINISHED_RUNS_KEPT` records read per page) are the reserved view `__run` answered by the leader (`Node::run_view`), one shared `run_json` for both; they show plaintext sizes and hashes of files only to those who may list the files.
- **Pipeline runs** (`component/pipelines.rs` apply, `node/component/compute/pipelines.rs` leader loop, `api/pipelines.rs` routes; tests `component/tests/pipelines.rs`, `pipeline_runs.rs`):
  A pipeline run lives in the app group of an app made from a release that registered the pipeline, compute-only apps included (their group gets an `init` entry with no outcome, `init_compute_only`, so it has the component executor that holds files).
  `PipelineRun` is a hidden row (`rows::CPIPE`, by run id) with the release's `version`, the pipeline's `hash` and the compute component, all fixed at start, with `modules` (reference to the module release's compute hash, copied from the release's pins), the bound `inputs` and `params`, the `caller`, `payer`, and per step a `StepState { status, runs, outputs, result, reason }`; the head keeps `active_pipelines` (at most `MAX_ACTIVE_PIPELINE_RUNS`, 8) and the last `FINISHED_PIPELINE_RUNS_KEPT` (64) finished ids.
  Three entries move it, all judged on the record and the entry alone: `pipelineStart` (pins every input, +1 hold; refused for a known id, a full app or an input that is not a held file), `pipelineStep { run, step, decision }` (a pending step takes its first decision only; `Start { runs, fan }` writes one `RunRecord` per run, id `<run>.<step>.<index>`, no result method, `RunRecord::pipeline` linking it with its `index`, and records `fan: { total, items, batch, allowed }`; `More { runs }` is accepted only while the step runs, only for the next indices below `total` and until failures pass `allowed`; a start the app's `MAX_ACTIVE_RUNS` cannot hold or after a cancel or a failFast stop changes nothing, one that can never work fails the step; `Failed` is also accepted for a running fan-out with runs left to create) and `pipelineCancel` (flags the run and its active step runs).
  `apply_compute_final` settles the step of a linked run in the same apply: outputs pinned (+1 each, released when the record is dropped), outputs and results kept per run index (`run_outputs`, `run_results`, so `<step>.outputs` is in item order), a result up to `MAX_STEP_RESULT_BYTES` and a step's together up to `MAX_STEP_RESULTS_BYTES` (past it none are kept and `results_too_large` is set), failed items counted from the runs that failed or were cancelled; the step is `succeeded` once every run it will have is final and `failed_items` is at most `fan.allowed` (zero while the run is cancelled or stopping), else `failed` (one item over cancels its active runs and no more are created), and the run ended once every step is settled (inputs released; `succeededWithFailures` when a step tolerated failed items). `PipelineRun::fail_fast` (the pipeline's `failFast`) makes the first failed step set `stopping`, end the pending steps `cancelled` and flag the running steps' runs.
  The leader's loop (`Node::drive_pipelines`, beside `drive_runs`) is stateless: `plan` decides every pending step from the records (start when its needs succeeded, `skipped` for no items, `upstreamFailed` when one failed, `cancelled` for a cancelled or stopping run) and the next `More` wave of each running fan-out (`maxParallel` less its active runs, the app's room, `MAX_STEP_RUNS`), and each decision is proposed only while it still applies (the step pending, or the fan-out's next index unchanged). A pipeline whose definition is gone (`pipeline_by_hash` answers 404) gets `Failed{PIPELINE_UNAVAILABLE}` for its steps that have not started or have runs left to create.
  A step in another package's module (`bundle_of`) is built with that module's caps (`caps_of_component` per bundle) and `StepRun::compute_bundle` (none for `self`); apply accepts only a bundle the run pinned (`UNKNOWN_MODULE` fails the step), writes it as the `RunRecord`'s `compute_bundle`, and the cache key uses it. Owner, payer, `bundle` (usage) and file keys stay the app's; the job node is pushed the module's component like any compute component. Regression: `a_step_runs_another_packages_module_under_the_callers_keys_and_payer`.
  It keeps only caches of immutable data, a pipeline by hash (`Node::pipeline_by_hash`) and a compute component's caps (`caps_of_component`).
  A step's args are its `args` with `params.x` replaced by the run's value or the default and `<step>.result` by that step's result as JSON; no args is no bytes; its inputs are `inputs.x` and `<step>.outputs`, deduplicated, at most `MAX_PIPELINE_RUN_INPUTS` (10,000, which the job node's admission and `MAX_ASSIGNMENT_BYTES` (8 MiB) allow; a run an app starts keeps `MAX_RUN_INPUTS`).
  A `forEach` step makes one run per batch of items; its inputs are the step's `inputs` references in the order written with the run's item files where `item` stands (first when it is not listed), and `item` in `args` is the item (a file id or a result value; a list for a batch above 1). `collect: <step>.outputs` is one run over all of that step's outputs in item order, then its other `inputs`. A step's `pool` is carried on `StepRun::pool` into `RunRecord::pool`; `AcquireRun { pool }` then puts the slot in that pool (`RUN_NO_POOL` when the cluster lacks it or its `maxMemoryMib` is below the run's memory), so `pick_job` uses only its job nodes. A step without `pool` keeps `pool_for`. The leader proposes `pool` only while `Node::nodes_behind_release` is empty (an older node would drop it), else `None`. Regression: `a_steps_pool_places_its_runs_on_that_pools_job_node`, `a_named_pool_waits_until_every_node_runs_this_release`.
  Only those who may handle the app's files (`api::may_handle_files`: its owner, its organization's members) start, cancel or retry a run, even on a public app (`403 FORBIDDEN`); anyone who reaches the app reads it without results or file hashes. Regression: `only_those_who_handle_the_apps_files_start_cancel_or_retry_a_pipeline`.
  The routes judge the request against the pinned pipeline first (`NO_SUCH_PIPELINE`, `BAD_INPUTS`, `BAD_PARAMS`, `TOO_MANY_PIPELINE_RUNS`, `PIPELINES_NEED_ONE_SERVICE`, `CLUSTER_NOT_UPGRADED`); start and cancel travel as the host commands `__pipeline_start` and `__pipeline_cancel` to the leader, reads as the reserved view `__pipeline_run` (`PIPELINE_VIEW`).
  A run with no result method ends with an empty result outcome and prepares nothing on the replica engine.
  **Receipts**: a linked run's final writes a `StepRunProof` (`component/proofs.rs`, hidden row `rows::CPROOF` by step run id: attempt, node, job key, assignment digest, SHA-256 of the signed report message, the job's signature, outputs, usage; no report fields for a leader-ended run) and the proofs go with the pipeline run's record when it is dropped (`drop_step_proofs`), so they outlive the `RunRecord`s (`FINISHED_RUNS_KEPT`). A cached run has no proof: `StepState::cached_keys` keeps its cache key. `PipelineRun::input_files` records each input's size and hash at start. `GET /apps/{app}/pipeline-runs/{run}/receipt` (`api/pipeline_receipt.rs`; `may_handle_files`, else `403`; `409 RUN_NOT_ENDED`) reads both through `__pipeline_run` with `receipt: true` and answers `{ receipt, node, signature }`: `calimero_client::receipt::PipelineReceipt` (`modules` is the module name to compute hash with `self` for the package's own, `pipelineHash` maps steps to modules), signed by `Node::sign_pipeline_receipt` over `receipt_message` (`"calimero.pipeline.receipt.v1\0" || JCS(receipt)`). Not yet: the client does not verify each job's report signature, and module logs are not in it. Tests: `component/tests/pipelines.rs`, `pipeline_runs.rs` (`a_finished_run_has_a_receipt_signed_by_an_attested_node`, `a_running_run_has_no_receipt_yet`, `a_fan_out_with_cached_items_marks_them_cached_in_its_receipt`, `a_stranger_cannot_read_a_receipt`).
  **Cost cap** (`PipelineRun::cost: CostCap { maxCost, priceVersion, table, worstCase }`, set by the start route from `sys_fresh`'s price table, `NO_PRICE_TABLE` without one; the table is copied so apply reads no system state): `PipelineRun::committed_cost` is the sum over steps of `StepState::cost` (priced usage of finished runs, `priced_cost`, added in `settle_step_run` even after the step ended) and, for a running step, its active runs (`runs - finished`) times `StepState::run_cost` (the worst case of one run, set when a wave starts).
  `plan` cuts each wave to what `Budget::afford` allows and, when the cap kept runs out that the wave would otherwise carry, adds `Failed{COST_CAP}` for the step in the same round; apply refuses a `Start`/`More` whose new runs' worst case would pass the cap (`COST_CAP`, nothing changes) and takes `Failed{COST_CAP}` on a running step as `StepState::capped`: no more runs, the active ones finish and are paid for, then the step fails with that reason.
  Cached runs cost zero; the view shows `cost`, `costSoFar` and each step's `cost` as decimal strings.
  **Retries**: `POST /apps/{app}/pipeline-runs/{run}/retry { limits?, maxCost? }` (`api/pipelines.rs::retry`) reads the ended run through the reserved view, refuses a running one (`STILL_RUNNING`), checks `limits` against the method's caps (`check_limits`: `BAD_LIMITS`, `LIMIT_EXCEEDS_CAP`) and starts a new run through the same `start_launch` as a start, with the original's `version`, `hash`, `computeBundle`, `inputs` and `params`.
  `PipelineStartEntry`/`PipelineRun` carry `retry_of` and `limits` (a `calimero_pipeline::Limits` per step, applied by `build_step` over the pipeline's own with `Limits::overridden_by`, then clamped to the method's caps); apply refuses limits for a step the run lacks.
  Failed items rerun because only done runs write cache entries and the key leaves caps out.
  Tests: `pipeline_runs.rs` (`a_retry_with_raised_limits_reruns_only_the_failed_items`, `a_fuel_failure_is_not_retried_automatically`, `a_lost_item_is_retried_and_billed_once`, `retrying_a_running_run_is_refused`, `a_retry_keeps_the_original_release_after_an_upgrade`).
  **Events**: `GET /apps/{app}/pipeline-runs/{run}/events` (`api/pipeline_events.rs`; SSE, the run view's gate, so who reads the run subscribes) sends `snapshot`, then `run` and `step` events, then `end` and closes.
  The events are `calimero_client::pipeline_events` structs read from the `__pipeline_run` view with `files: false` (`RunProgress::from_view`), never a filtered copy of the record.
  The source is the strong view, polled every `PIPELINE_EVENTS_POLL` (500 ms) and diffed, so a follower answers through the leader as `GET .../pipeline-runs/{run}` does; access is judged when the stream opens.
  Bounds: `MAX_RUN_SUBSCRIBERS_PER_APP` (64 per node, `429 TOO_MANY_SUBSCRIBERS`, `Node::pipeline_subscribers`), keep-alive every 15 s, and `PIPELINE_EVENTS_MAX_SECS` (1 h).
  Tests: `pipeline_events::tests` (client; key sets, diffs), `pipeline_runs.rs` (`a_subscriber_sees_every_step_move_and_the_end`, `events_never_carry_results_or_file_ids`, `a_stranger_cannot_subscribe_to_a_private_apps_run`, `subscribers_are_bounded`).
  **Validate and estimate** (`api/pipelines.rs`; reads only, nothing logged): `POST /pipelines/validate { source, package?, version? }` (a signed-in account) answers `{ ok, errors: [{ path, code, message }], pipeline?, worstCaseFuel?, worstCaseCost? }` through `Node::validate_pipeline`, which compiles and, for a release the caller sees (`sees_release`, else `404`; one of `package` and `version` alone is `400 BAD_RELEASE`), checks against the `Env` install builds (`packages/pipelines.rs::env_for`: the release's compute methods as `self`, `resolve_modules` for the caller, `pool_ceilings`) and prices with the price table read through `sys_fresh`.
  `POST /apps/{app}/pipelines/{name}/estimate { inputs? | counts?, params? }` (`may_run`, the start's gate) answers `calimero_pipeline::estimate` over the pinned pipeline and the run's modules as decimal strings with `priceVersion` when a table exists; input ids are only counted, never checked for being held (`400 BAD_INPUTS` for both forms at once or an undeclared input).
  Tests: `pipeline_runs.rs` (`validate_reports_every_error_with_its_path_and_writes_nothing`, `validate_against_a_release_uses_its_methods_and_caps`, `validate_a_release_the_caller_cannot_see_is_404`, `estimate_follows_the_input_count`, `a_stranger_cannot_estimate_on_a_public_app`).
  **Step cache** (`component/step_cache.rs`): a hidden row family `rows::CCACHE` keyed by cache key holds `CacheEntry { outputs, result, ts_ms, source_run }`, the head keeps the keys oldest first (`step_cache`).
  The leader makes the key (`cache_key`: SHA-256 of the JCS of compute bundle, method, args hash, each input's plaintext SHA-256 in order; not caps, so a retry with raised limits still hits) in `build_step`, puts it in `StepRun::cache_key` (none for `cache: false` or an input of unknown content), and apply never recomputes it; `RunRecord::cache_key` carries it, and a linked run that ends done writes or refreshes its entry (a result past `MAX_STEP_RESULT_BYTES` is not cached) and pins its outputs for the entry's life.
  `plan` puts each run the cache answers (`CacheView::hit`: an entry whose outputs are all held) in `Start`/`More` `cached: [{ index, key }]` instead of `runs`; hits cost no room, and a wave ends at the first miss past its size.
  Apply takes the hits as finished runs in index order (`StepState::cached_runs`, outputs and results by index, `finished` counted, no `RunRecord`), settling the step at once when no run is left; a hit whose entry or outputs are gone ends the wave there (`CACHE_GONE` if it is the first), so the leader decides that index again.
  Entries leave past `MAX_CACHE_ENTRIES` (oldest first) and after `CACHE_TTL_MS` by the applied entry's timestamp (`expire_step_cache`, at most `CACHE_EXPIRY_STEP` per entry), releasing their holds; a hit refreshes the entry and moves it last.
  `StepState::is_cached` and `cached_items` feed `cached` and `cachedItems` in the run view; a start naming a file id the app does not hold answers `410 OUTPUT_EXPIRED`.
  Tests: `component/tests/pipelines.rs` (apply), `compute/pipelines.rs` (`plan` keys and hits), `pipeline_runs.rs` (`rerunning_with_one_changed_param_runs_only_the_steps_it_reaches`, `a_step_with_cache_false_always_runs`, `a_fan_out_reuses_its_cached_items`, `starting_with_an_expired_output_answers_output_expired`).
- **Transfers**: `POST /apps/{app}/transfer { to }` (owner, or for an org app its owner while a member or an org admin, `SystemState::manages_app`; passkey consent; an org app only to a member). Regression: `an_owner_transfers_an_app_with_their_passkey`.
- **Moves**: `POST /apps/{app}/move { toOrg, bringMembers }` (`SystemCmd::MoveApp`, `SystemState::move_app`; consent `<app> / __move`): the app's manager where it is now, as an admin of `toOrg`. Data stays put; `AppRecord.org`, the payer's file bytes and compute slots move, within the target's quotas. `org`/app-team access becomes the target's app team (made open, or closed when every app team of the target is; `org` access in a legacy target); a team other than the package's app team is refused (`MOVE_NEEDS_APP_TEAM`), and so is an owner who is not a member of the target afterwards (`MOVE_OWNER_NOT_IN_TARGET`). `bringMembers` adds the source's members who reach the app, capped at member, with app-team rows. Regressions: the `moving` unit tests in `system.rs`, `tests/move_app.rs`.
- **An org app's owner, while a member** (`SystemState::owner_acts`): an organization's app gives its owner the owner's powers (access, metadata, transfer, delete, upgrade, upgrade keys, files, blob deletes, usage) only while they are a member (`member` or above) of its organization; its admins keep transfer and delete (`manages_app`). Judged at apply; `SetAccess`, `SetAppMetadata`, `TransferApp`, `DeleteApp`, `UpgradeApp` and `CreateApiKey` carry `owner_in_org`, which the API always sets; absent in an entry an earlier release logged, which replays judging ownership alone. Regressions: `an_owner_removed_from_the_organization_no_longer_manages_its_app`, `a_member_owner_and_a_personal_owner_still_manage_their_apps`, `system::tests` (`an_org_apps_owner_manages_it_only_while_a_member`, `an_org_admin_transfers_or_deletes_an_app_whose_owner_left`, `app_commands_logged_before_the_membership_rule_replay_as_they_did`).
- **An app on a team, its owner while in the team** (`SystemState::owner_acts`, `may_reach`): an app with `team` access is reached and managed by its owner only while they are in that team (`Org::in_team`), so an owner kicked from, leaving, or closed out of an app's team (or a private channel) loses the channels they created there; admins transfer them. The six owner commands above and `ScopeApps` (which finds who reaches an instance) carry `owner_in_team`, which the API always sets; absent, an entry replays judging the owner without the team (`OwnerRule`). Regressions: `an_owner_out_of_the_apps_team_no_longer_reaches_or_manages_it`, `system::tests::an_apps_owner_reaches_and_manages_an_app_on_a_team_only_while_in_it`.
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
  and status, error codes, latency per route group and body bytes; WASM run
  time by kind, `calimero_wasm_run_duration_seconds{kind}` from
  `metrics::wasm_run` in `Node::metered` and the view and action runs; a
  logged call's `run` and `commit` time,
  `calimero_call_phase_duration_seconds{phase}`, observed in
  `call_local_component` once the entry is appended), never
  names or state: labels are templates, groups, codes, kinds, phases and
  numbers, and ids only on the usage series below
  (regressions: `metrics_count_requests_errors_and_raft_without_naming_anyone`,
  `wasm_runs_and_a_calls_run_and_commit_are_timed`). `--metrics-listen` (`Config::metrics_listen`) serves the same text
  at plain-HTTP `GET /metrics` for a scraper, nothing else (`api::plain_metrics`).
- **Usage metering** (`usage.rs`, `Node::usage`; billed from `/metrics`):
  `calimero_app_{calls_total{kind},wasm_cpu_seconds_total,wasm_memory_byte_seconds_total,outbound_requests_total,state_bytes,blob_bytes}`,
  labelled `app`, `org` (the payer, whose quotas it counts against: org id,
  or `account:{owner}` for a personal app; `usage::payer`) and `developer`
  (who Calimero Cloud bills: for an instance of a deployment the deployment's
  owner, in the same form, else the payer; `usage::developer`,
  `usage::Billed::of`). Both are functions of the app, so they add no series
  (at most 8 per hosted app). Regression for developers:
  `an_instances_usage_is_billed_to_its_deployments_developer`. Every WASM run goes through `Node::metered`
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
  `report::MAX_REPORT_PAGE`). Per app: scope, bundle digest, package release
  (`report::release_of_app`: the one the instance runs, so releases sharing a
  WASM are told apart),
  `upgradesHeldBy`, `lastUpgrade` (`AppRecord::last_upgrade`, with
  `report::actor_kind`: `owner`/`member`/`operator`, judged against the app's and
  org's owners as they stand), approved releases, each replica's release and
  whether approved; for accounts also name, the upgrader's account, newer
  visible releases (`availableVersions`, from the release it runs, those with
  its WASM included) and `clusterReleases`. `lastUpgrade.from`/`to` name each
  end's release from `UpgradeRecord::from_release`/`to_release` when recorded. The operator
  sees a package name only for releases it installed (`Reader::Operator`,
  `sees_release`), never an account, a name or an upgrade's error text.
  Node releases come from `Node::release_of_node` (attested once, then kept;
  at most `PLACEMENT_RELEASE_WAIT` each), as for placement. Every report also
  names each app's `lastWriteAt` (Unix seconds, `null` before its first write;
  `api::with_last_writes`): `ComponentHead::last_write_ms`, read from this
  node's replica or, for an app it does not host, a replica's
  (`Node::last_write_ms`, `internal/last-write`, at most `LAST_WRITE_WAIT`
  each, one page at a time). Regressions:
  `app_versions_and_release_approvals_are_reported_without_names`,
  `an_operator_upgrade_is_reported_as_the_operators`,
  `an_apps_last_write_is_the_same_on_every_node_and_survives_a_restart`.
- **Forwards are bounded**: a call or read forwarded to an app's leader waits at most
  `FORWARD_TIMEOUT` (5 s, `peer_forward`), then retries at whoever leads now; a frozen
  leader would otherwise hold a write for the 30 s HTTP timeout. Calls without a
  client key get a node key (`node:…`, kept 10 min) first, so a retry replays a call
  that did commit. System-log forwards keep the long timeout: some system commands
  are not idempotent yet. Regression: `frozen-leader` in `tests/fault/suite.py`.
- **Leader races**: `Group::propose` takes no leadership round before a call's
  append, so a deposed leader may run a call on stale state; its entry never
  commits, or commits as a no-op (`base_index` ≠ the app's last entry,
  `AppState::last_index`: `conflict`, no idempotency record), and `Node::call`
  re-runs it instead of failing. The base is the app's last entry, never the
  log's applied index, which counts blank and membership entries too: with it
  the first call after every election conflicted once (regression:
  `the_first_call_after_a_leader_change_commits_without_a_rerun`;
  `calimero_build_conflicts_total` in `/ops/metrics` counts conflicts). Replies without an append (idempotent replays) are confirmed first.
  The component is prepared from the bundle read before the lock, so the call
  also answers a conflict when the state names another bundle by then (an
  upgrade applied in between; `call_component` compares `AppState::bundle`).
- **Runs leave the async workers** (`node::off_workers`): a WASM run (call,
  timer tick, init, strong view) happens inside the group's
  proposal lock, on the machine's read lock, from borrowed state, so it cannot
  move to `spawn_blocking`; it goes through `block_in_place`, which hands the
  worker's other tasks (Raft heartbeats, the system group, HTTP) to another
  worker meanwhile. Regression: `a_long_run_does_not_hold_an_async_worker`.
- **Upgrades are code-only**: a component upgrade swaps the bundle
  (`componentUpgrade`); a named `migrate` is refused `MigrationsNotSupported`. The
  consent page shows the state version each side's `calimero:abi` declares and flags a
  change (`stateVersion`), as no migration runs. The directory records the new bundle only
  once the upgrade succeeded.
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
  `Org::member_metadata` (the member or an admin), `AppRecord::metadata` (owner,
  while a member for an org app, or org admin), each at most `MAX_METADATA_BYTES` (16 KiB, team metadata too);
  `Account::passkey_labels` (`POST /auth/passkeys/{cred} { label }`).
  `DELETE /orgs/{org}` is the owner's, with a passkey assertion (412 first),
  and takes the org's teams, invites and apps. Ownership moves with
  `SetOrgRole { role: owner }`; leaving is `RemoveOrgMember` on oneself.
- **Organization recovery** (`docs/specs/org-recovery.md`; `Org::recovery`,
  `types::OrgRecovery`): an org may hold a recovery key (Ed25519 public half;
  the owner keeps the seed). `SetRecoveryKey` (owner, consent `__set_recovery_key
  { publicKey }`; a new key or `null` ends a pending recovery), `StartRecovery`
  (any account, with the seed's signature over org, account and nonce,
  `calimero_identity::recovery`, checked against the org's current key in
  `authorize` and spent), `CancelRecovery` (owner, `__cancel_recovery
  { account }`), `FinishRecovery` (the named account, `__finish_recovery
  { org }`, at least `RECOVERY_DELAY_SECS` (72 h) after the start, judged at
  the commands' `at`: the node's clock, `Node::app_clock`, so tests move it).
  Finishing removes every previous owner from the org and its teams and moves
  their org apps to the new owner; account credentials are never touched. One
  pending recovery per org (`409 RECOVERY_PENDING`); `403 WRONG_RECOVERY_KEY`
  for no org, no key or a wrong signature alike. Routes: `POST
  /orgs/{org}/recovery`, `.../recovery/{start,cancel,finish}`. Regressions:
  `tests/org_recovery.rs`, `recovery_needs_the_owners_passkey_the_seed_and_the_delay`,
  `rotating_the_recovery_key_retires_the_old_seed`,
  `state_from_before_recovery_reads_with_none_and_writes_as_before`.
- **Data policy** (`policy.rs`, `Org::data_policy`; docs: protocol organizations "Data policy"): an org's readers, recipients, sources and sinks as chained versions (`version`, `prev` = SHA-256 of the previous RFC 8785 form), approved only by admins' passkeys: `ProposeDataPolicy`, `ApproveDataPolicy`, `WithdrawDataPolicyProposal` (`org:<org> / __propose_data_policy | __approve_data_policy { hash } | __withdraw_data_policy`), and `ApproveDataPolicyByRecovery` (the recovery key over `policy::RECOVERY_SUBJECT`, for a first version, or a later one whose current signers still admins cannot reach its threshold). No operator path. First version needs two admins, later ones `current.threshold` of `current.signers`; storage and signing only, enforcement reads `OrgDataPolicy::current`. Routes `GET|POST /orgs/{org}/data-policy`, `.../approve`, `.../withdraw`, `.../recovery-approve`; `meroctl namespace data-policy`. Regressions: `system::tests` (`a_first_policy_needs_two_distinct_admins` and the tests around it), `tests/org_data_policy.rs`.
- **Account recovery** (`docs/specs/account-recovery.md`; shaped like
  organization recovery above, same `RECOVERY_DELAY_SECS`): an account may
  hold one recovery code (`calimero_identity::new_recovery_code`, Crockford
  base32, `normalize_recovery_code`, `MAX_RECOVERY_CODE_CHARS`); the log keeps
  only its Ed25519 public key (`proof::recovery_key`, as invites;
  `SystemState::recovery_keys`, key -> account). `SetAccountRecoveryKey`
  (`POST /auth/recovery`, unbound passkey session, `consent_minted`
  `account:<a> / __set_recovery { recoveryKey }`, the code answered once),
  `RemoveAccountRecoveryKey` (`DELETE /auth/recovery`, `__remove_recovery
  {}`). Signed out, `POST /auth/recovery/begin { code }` then `/start
  { challengeId, code, registration }` (UV, the cluster's own origins):
  `StartAccountRecovery` carries the code key's signature
  (`proof::sign_recover` over account, credential id, SHA-256 of its public
  key and `at`, checked in `authorize` and spent), spends the code, and
  records `SystemState::account_recoveries[a] = types::PendingAccountRecovery
  { credential, startedAt, readyAt }`; no passkey, no session. One per
  account (`409 RECOVERY_PENDING`). `CancelAccountRecovery` (`POST
  /auth/recovery/cancel`, a passkey the account has, `__cancel_recovery
  { credentialId }`). `FinishAccountRecovery` (`POST /auth/recovery/finish
  { accountId? }`, `412`, `Pending::RecoverFinish`): the pending passkey's
  assertion over `session / __sign_in / sign_in_args` of the new refresh
  token (`api::NewSession`, as `CreateRefresh`), verified at apply under that
  credential; at `at >= readyAt` (the node's `app_clock`, `409
  RECOVERY_NOT_READY` before) it adds the passkey (counter from the
  assertion), revokes every session of the account
  (`SystemState::revoke_account_sessions`) and records the refresh token.
  `/auth/me`: `recoveryCode`, `recoveryPending`. Regressions:
  `tests/recovery.rs`, `a_recovery_code_is_set_and_removed_with_the_accounts_passkey`,
  `an_account_recovery_starts_only_with_the_codes_signature_and_the_account_cancels`,
  `an_account_recovery_finishes_after_the_delay_with_the_new_passkey`,
  `state_from_before_account_recovery_reads_and_writes_as_before`.
- **Devices**: a person is their account, and each passkey is a device: the
  session names the one it was opened with (`SessionClaims::passkey`,
  `passkeyId` in `/auth/me`); `auth/passkeys/add` adds one.
- **Passkey times and sessions** (the account holder's alone): `Account::passkey_times`
  (`types::PasskeyTimes { addedAt, lastSignInAt }`, by credential) and
  `RefreshRecord::opened_at` are set at apply from the times the entries carry:
  `CreateAccount::at` (added, signed in, session opened), `AddPasskey::at`,
  a sign-in's `CreateRefresh` record `opened_at` (last sign-in, the latest
  wins), a recovery finish's record `opened_at`; `UseRefresh` keeps the old
  record's `opened_at`, so a refresh is no sign-in and no extra entry is
  written. Each is outside what a passkey signs (`account_args` and
  `sign_in_args` are unchanged, and every field is left out when empty), so
  a replica on the earlier release checks the same consent and ignores
  them; an earlier release's entries carry none and leave the times
  unknown (`null`) on every replica. `/auth/me` gives `addedAt` and
  `lastSignInAt` per passkey; `GET /auth/sessions` (`api::account_sessions`,
  `MAX_SESSIONS_LISTED`) lists the caller's live sessions with
  `lastActiveAt` = the last exchange (`expires_at - REFRESH_TTL`, up to
  about 1 h 15 min behind), and `DELETE /auth/sessions/{session}` ends one
  (`RevokeSession`; `404` for any session the account does not hold live).
  Both are `holder` (unbound passkey session), as `/auth/logout-all`.
  Regressions: `tests/sessions.rs`,
  `passkey_and_session_times_come_from_their_entries`,
  `accounts_and_sessions_from_before_the_times_read_and_write_as_they_did`,
  `tests/old_datadir.rs`.
- **Members' security** (owner and admins): `GET /orgs/{org}/members?detail=security`
  (`api::org_members`, `holder`, `SystemState::is_org_admin`, filtered by
  `Org::sees_member`) answers per member the passkey count capped at
  `PASSKEYS_COUNTED` (`"3+"`), `recoveryCode` (a `recovery_keys` entry),
  `lastActiveDay` (`api::last_active`: the latest `PasskeyTimes::last_sign_in_at`
  or live session exchange, `api::session_use`, as a UTC day) and `sessions`
  (live, unrevoked, each session once). Nothing the holder alone sees (ids,
  labels, sessions, origins, activity). `deployment_orgs` adds counts only,
  `memberCount` and `activeMembers30d` (`ACTIVE_WINDOW_SECS`). Regressions:
  `tests/org_member_security.rs`, `deployments_of_one_release_are_independent`.
- **Activity** (the account holder's alone): `SystemState::activity` (by
  account, beside `accounts` so `account_args` is unchanged), the last
  `types::ACTIVITY_KEPT` `ActivityEntry { at, passkey, kind }`, `kind`
  `signIn { app, origin }` (from a session's opening record: `CreateRefresh`,
  `CreateAccount`, `FinishAccountRecovery`, via `system::sign_in_of`) or
  `approval { method, subject }` (one hook in `SystemMachine::run`:
  `system::approval_of` finds the consent `authorize` verified, by its used
  marker, before the command applies, and records it after). The time is the
  step-up's `proof::Consent::at`, outside what the passkey signs; 0 (a
  sign-in's consent, or an earlier release's) records nothing, so a replayed
  old log adds nothing. `GET /auth/activity` (`api::activity_list`) is
  `holder`. Regressions: `tests/activity.rs`,
  `activity_records_sign_ins_and_step_ups_from_their_times`,
  `tests/old_datadir.rs`.
- **Passkeys are per cluster** (`api/passkeys.rs`; docs: protocol identities
  "Passkeys are each cluster's own"): clusters share an rp-id, so a
  registration's `user.name`/`displayName` is `passkey_user_name` (person ·
  `config.cluster` · `cluster_tag`, the first 2 bytes of the cluster identity
  key, public in `GET /sign-up` as `passkeyTag`; plus the app's name for a
  sign-up begun on a package origin), every registration's user handle is
  random (a shared one makes a password manager replace the earlier passkey;
  `each_added_passkey_gets_a_user_handle_of_its_own`), and an assertion with a credential no account has answers `401
  PASSKEY_UNKNOWN_HERE` (`unknown_here`, from `check_assertion_for`), naming no
  other cluster or account. Regressions:
  `a_passkey_from_another_cluster_is_named_for_its_own_and_refused_here`,
  `a_registered_origin_signs_people_in_to_its_package_only`.
- **Init**: `POST /apps` takes `initArgs` (JSON) for the app's `init`, kept on the
  `AppRecord`; a failed init answers `422 INIT_FAILED` (`AppState::init_error`).
- **Ids people type** (accounts, apps, organizations, teams, upgrade runs, API
  keys) are base64url that never starts with `-` (`api::typed_id`), which
  meroctl would read as a flag. Regression: `a_typed_id_never_starts_with_a_dash`.
- **Context ids**: an app answers to its context id (`hex(SHA-256(app_id))`, what the app
  sees as `env::context_id()`) wherever the API takes an app id (`Node::canonical_app_id`).
- **Blobs** (`POST/GET /apps/{app}/blobs[/{id}]`; how an app holds one: see
  "Component executor"): bytes live in the cluster's blob store (`hex(SHA-256)`, a
  majority before the answer, a missing one fetched from a peer). A download asks the
  app's leader whether the app still holds the blob (`blob_held_strong`; 503 when no
  leader answers). `DELETE /apps/{app}/blobs/{id}` (owner or org admin) forgets it, then
  `Node::collect_blob` deletes the bytes on every node (`internal/blob/delete`) unless
  another app still holds the same content (asked of each app's leader; an app whose
  leader does not answer counts as holding it) or it is a bundle or a release `.mpk`.
  Every path that stores bytes before their record commits pins the ids on the app's leader first,
  and holds the pin until the record commits or fails (`node/blob_guards.rs`): `logged_entry` (a
  publish, a call's files, a run's outputs; it runs on the leader) and the plain upload
  (`Node::pin_on_leader`: locally on the leader, else through `internal/blob/pin`, a lease
  that expires after `LEASE_TTL` if the receiving node never releases it). Every deletion of bytes
  (`collect_blob`, which the owner's delete, the queued collection and discarded files call, and a
  peer's `internal/blob/delete`) asks `Node::blob_in_use`: held or pinned, as each app's leader
  answers, unknown counting as in use. A put of an id (`BlobGuards::put`) also stops a deletion of
  that id that was between its checks and its delete, here and on every peer. Regressions:
  `compute_runs.rs` (`a_blob_stored_again_after_a_delete_survives_whichever_nodes_handle_it`, a
  matrix over the kind stored, the node it goes through and the node the delete goes through;
  `a_collection_between_a_publishes_push_and_its_commit_leaves_the_bytes`,
  `a_peer_asked_to_delete_between_a_publishes_push_and_its_commit_keeps_the_bytes`,
  `a_peers_delete_that_checked_before_a_publish_stored_its_bytes_keeps_them`).
  A leader change between the pin and the commit is not covered: the pin stays on the old leader.
  A blob past `MAX_BLOB_BYTES` (47 MiB) is refused
  `413 TOO_LARGE` by the node, one past the sealed cap (64 MiB) by the envelope, in the
  clear; the client reports both as `TOO_LARGE`. Regressions:
  `a_blob_too_large_to_reach_the_other_nodes_is_refused_up_front`,
  `an_upload_past_the_sealed_cap_is_answered_413`.
- **App frontends** (`app_host.rs`): with `--apps-domain D`
  (`merod local`: `localhost`), each package's frontend is at
  `{label}.D` (`app_host::label`, 52 base32 characters), from its release's
  `.mpk` (`Node::release_mpk`, verified before a byte is served, opened ones
  cached). Outermost layer: only `GET`/`HEAD` of files, `/` → newest release,
  `/app/{app}/` → the release that instance runs (`AppRecord::release`; for a
  record from before, the newest release, by `compare_versions`, shipping its
  WASM), `/d/{deployment}/` →
  `/{version}/?deployment={id}` of the release it offers (looked up again on
  fresh state when nothing matches); every redirect keeps the request's query
  (after the `deployment` it adds), so `/?invite=...` reaches the page with its
  token (regressions: `frontend_redirects_keep_the_query`,
  `an_instance_link_opens_the_release_that_instance_runs`),
  client routes and relative tails served as the bundle's, its CSP plus the cluster's API
  origins and the attestation mirror (`ATTESTATION_MIRROR`, where clients get the release's
  measurements and Intel's collateral; regression
  `a_served_page_may_reach_the_cluster_and_the_attestation_mirror`) in `connect-src`, `/.well-known/calimero.json` (with the deployments
  offering the package); an unknown label under
  D is a 404, never a node route. Each package is also at
  `{cluster_label(config.cluster, package)}.D` on its cluster, the same site
  and origin (for mdma's hostname resolver; another cluster's label 404s;
  regression in `app_frontends_are_served_and_their_sessions_are_bound`,
  vector in `cluster_labels_match_the_resolvers_vector`). Each deployment has its own host too,
  `{deployment_label(id)}.D` (`app_host::Site`): the one package it offers
  (404 while it offers none or several), `/` → what `/d/{deployment}/` gives,
  `/d/` only for itself, well-known `deployment` set and listed first; the
  deployment API answers its `host`. Regression:
  `each_deployment_serves_its_frontend_on_its_own_host`. App origins (both
  kinds) pass CORS, and a passkey ceremony
  from one opens a session bound to its package (`SessionClaims::app`, carried
  by refresh): `Principal::within` keeps it to that package's apps (gate, app
  reads and writes, listings), `unbound`/`holder` refuse it credentials, API
  keys, approvals, upgrades, uninstalls and uploads, and it creates instances
  of its own package only. Its step-ups come from its own origin only.
- **Registered origins** (`origins.rs`, `api/origins.rs`; docs: protocol
  identities "Registered origins"): a package's manager
  (`SystemState::manages_package`: the bound publisher's org-wide
  `MANAGE_APPLICATION` holders, else `PackageRecord::owner`) registers a web
  origin of their own (`GET|POST|DELETE /package-origins/{package}`;
  `AddPackageOrigin`/`RemovePackageOrigin`, passkey consent `package:{p} /
  __add_origin / { origin, rpId }`, `__remove_origin / { origin }`, checked in
  `authorize`). Each carries its own rp-id (the host or a parent; never the
  cluster's rp-id or a parent, `origins::covers_cluster`). `api::app_origin`
  is the one lookup (served first, then registered) for ceremonies
  (`claimed_app`, `rp_with`: a registered origin alone under its rp-id), CORS
  (`lib.rs`) and the consent page's `returnTo`; begin routes take `origin`
  (`403 ORIGIN_NOT_REGISTERED`). Sessions opened there are bound like a served
  frontend's and name the origin (`SessionClaims::origin`,
  `RefreshRecord::origin`, kept by a sign-up's `Pending::SignUp` and stated
  nowhere in the sign-in challenge, like `app`); `principal` and `refresh`
  refuse them once it is removed, and `UseRefresh` refuses an exchange that
  changes it. A sign-up there answers its `412` with the origin's rp-id and
  takes the confirmation from that origin only; the sign-up policy judges it
  like any other (`a_registered_origin_signs_up_only_whom_the_cluster_takes`). `UninstallPackage` of the last release forgets them. Limits
  `MAX_ORIGINS_PER_PACKAGE`, `MAX_PACKAGE_ORIGINS`; `http` only for
  `localhost` names on a `--dev` node (`allow_package_uploads`). Regressions:
  `tests/package_origins.rs`, `package_origins_carry_their_managers_consent`,
  `package_origins_are_unique_bounded_and_go_with_their_package`,
  `state_from_before_package_origins_reads_with_none_and_writes_as_before`.
  **Served origins** (custom domains; design `docs/specs/custom-domains.md`):
  `served: true` on the body, `PackageOrigin::served` and
  `AddPackageOrigin::served` (both left out when false), consent
  `{ origin, rpId, served: true }` (`origins::add_args`, the one builder for
  the API and `authorize`). `app_host::layer` serves the package's site on a
  `Host` whose origin (`served_origin_of`: `https://name` on 443, or
  `http://name.localhost:port`) is a served one (`Node::served_site`, after
  `app_host_site`; never under the apps domain or on a node's API host).
  Ceremonies, CORS, binding and removal are the registered origin's, unchanged
  (`app_origin` answers it with its rp-id; `app_origin_package` stays
  apps-domain only). The API refuses (`served_fits`) a port, no
  `--apps-domain` (`NOT_SERVING`), a name under the apps domain or the
  cluster's rp-id, a node's API host, a held origin with the other `served`;
  limits `MAX_SERVED_ORIGINS_PER_PACKAGE` (4), `MAX_SERVED_ORIGINS` (64, the
  certificate metadata budget); `require_upgraded_nodes` before the consent,
  so no older node drops the flag. `GET /ops/served-origins` (ops token)
  lists `{ origin, host, label, addedAt }`, no package names, for mdma's
  certificates and resolver routes. merod terminates no TLS: Traefik in the
  TD does, with certificates mdma delivers. Regressions:
  `a_served_origin_serves_its_packages_frontend_and_binds_its_sessions`,
  `served_origins_carry_their_own_consent_and_limit`,
  `origins_from_before_served_ones_read_unserved_and_write_as_before`,
  `a_served_origin_is_an_https_host_without_a_port`,
  `a_custom_domains_host_names_one_origin`.
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
  sheet is `consent_page::diff`, its methods, events, timers and state version from the
  `calimero:abi` each component embeds, `component::embedded_abi`; regression:
  `an_upgrades_page_names_what_the_components_abis_change` with
  `apps/v2-fixtures/notes-v1` and `notes-v2`; the plain-language `title`, `points` and
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
  `GET /apps/{app}/storage` (keys, bytes, blobs, `lastWriteAt`), `GET /apps/{app}/abi` (the
  bundle's embedded ABI), `GET /auth/usage` (the account's apps, bundle bytes,
  API keys, owned orgs, the quotas, and `developerFuelToday`/`developerWritesToday`:
  today's runs of the apps the account is the developer of), `GET /orgs/{org}/usage`
  (the same figures for the apps an organization is the developer of, its admins
  only, D78), `GET /apps/{app}/usage` (a component's runs, owner and org admins only;
  see "Usage" below).
- **Teams** (`types::Org`): nested (`parent`, moved with
  `UpdateTeam`), open (`TeamJoin::Everyone`: whoever reaches the parent, the
  organization at the top, is a member, unless denied `caps::JOIN_OPEN_TEAMS`)
  or restricted (direct rows only). A direct row has a role (`TeamRole`: lead,
  member, read-only) and capabilities (`caps`, `MemberCapabilities` bits;
  the organization keeps a default). An app deployed with `access: team` is
  reached by the team's members; a member whose direct row there is read-only
  runs only its views, any other call refused before it runs (`403`,
  `node::READ_ONLY`), so no run burns the developer's fuel uncounted (regression:
  `a_read_only_members_call_is_refused_before_it_runs`). A team's creator leads it. Below admin, creating an app in the organization needs `caps::CREATE_APP` (on its team, or org-wide) and minting an invite `caps::INVITE_MEMBERS` (org-wide, or on each team it names); neither is in `caps::DEFAULT`, and a team's leads hold both there. `caps::MANAGE_APPLICATION` lets a member upgrade the org's apps (on the app's team, or org-wide), start org upgrade runs, and approve the org's outbound hosts (org-wide grants only); a personal app stays its owner's (`SystemState::manages_apps`; regressions: `a_member_needs_the_capability_to_create_apps_or_invite`, `manage_application_lets_a_member_upgrade_and_approve_outbound`). A member who left, or
  whom a manager removed, does not come back by inheritance until a manager adds them again or an invite names the team (`Team::removed`; regression: `leaving_an_open_team_sticks_until_someone_lets_you_back_in`). Reads are filtered: a
  team one neither reaches nor manages is not listed (`GET /orgs/{org}`) and
  answers `404` (`GET /orgs/{org}/teams/{team}`). `DeleteTeam` takes the teams
  below it and their apps. Metadata (`POST /orgs/{org}/teams/{team}`) is the
  app's (a folder's name and colour). A team's `default_capabilities`
  (`Team`, left out when unset) count for everyone in it without a row of
  their own, on top of their org-wide caps (`Org::team_caps`, used by `may` and
  open-team inheritance; a new row takes them too); set by `SetDefaultCapabilities
  { team }` (`POST /orgs/{org}/capabilities { team, capabilities }`, the team's
  managers or `MANAGE_MEMBERS`). In an org with app teams a guest inherits no
  open top-level team and reaches no `Access::Org` app (`Org::app_only`, used
  by `in_team_at` and `may_reach`); a legacy org's guests are as before.
- **App teams and modes** (`Team::app_package`: `metadata.app.package` on a
  top-level team; `Org::app_team`, `Org::mode` = legacy | standalone | shared,
  derived): one per package per org (`UpdateTeam` refuses a second,
  `APP_TEAM_EXISTS`); making, changing or dropping the mark, scoping apps and
  creating the first instance of a package with no app team in an org that has
  app teams are admin-only (`ONLY_ADMINS_INSTALL`). `ScopeApps` (`POST
  /orgs/{org}/app-teams`, admin, passkey) finds or makes each package's team
  (id from the entry's `nonce`, `app_team_id`), gives a row to every member who
  `may_reach` the instances first, moves them to `Access::Team`, and with
  `shared` (default: two app teams or more) closes every app team after
  writing its inherited people as rows and drops `INVITE_MEMBERS` from its
  default (standalone teams it makes default to `INVITE_MEMBERS`); bounded by
  `MAX_SCOPE_PACKAGES`, `MAX_SCOPE_INSTANCES`, `MAX_APP_TEAM_NAME_CHARS`.
  `GET /orgs/{org}/app-teams` (any member; a guest only theirs). `GET
  /orgs/{org}` filters members, capabilities, member metadata and team rows by
  `Org::sees_member` (admins all; members all unless shared, then co-team
  people and admins; guests co-team people). `SetAccess` is also open to
  `SystemState::may_set_access` (`MANAGE_APPLICATION` where the app is and where
  it goes, org-wide for non-team access). Invites record `created_by` (set at
  apply) and `created_at` (both left out when unset); `GET /orgs/{org}/invites`
  and `DELETE /orgs/{org}/invites/{key}` (`RevokeInvite`) for whoever
  `SystemState::sees_invite`. Regressions: `tests/org_app_teams.rs`,
  `invites_teams_and_default_caps_from_before_app_teams_read_and_write_as_before`.
- **App state on disk** (`app.rs`, `component.rs`): `AppMachine` holds no state in
  memory but its small `Head` (bundle, executor, the component's host records). The
  app's keys are hidden rows of its group (`rows::CKV`), idempotency records too
  (`rows::CIDEM`), events a ring of `EVENT_RING` (`rows::EVENT`). The system group
  stays in memory (`Machine::IN_MEMORY`). Backups (`Node::backup`) copy the group's
  sealed snapshot file; `backup::latest` opens one with K_c.
- **Fresh system reads**: `Node::sys_fresh` is a read index (the leader names
  its applied index, `internal/system/index`; this replica waits for it). Org
  and team reads use it, and so does an access check before it refuses.
  Concurrent callers share a round (`ReadRounds`): one that arrives while a
  round runs waits for the next one, never takes the one already under way.
  One round per request: `principal` (or `gate`, anonymous) keeps the index its
  round named (`Principal::sys_index`, `Node::sys_fresh_index`); the request's
  later checks and the app leader's (`CallRequest::sys_index`, carried on
  `internal/call`) wait to apply it (`Node::sys_fresh_from`, whose doc is the
  argument that this is as fresh as a round of their own) instead of running
  another. Counted in `calimero_system_read_rounds_total` (`Node::sys_rounds`).
  Regressions: `a_call_costs_one_system_read_round`,
  `a_revocation_committed_before_the_round_holds_on_a_leader_behind`
  (`tests/access_gate.rs`).
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
- **Rollback counter** (`rollback.rs`, `docs/specs/rollback-protection.md`): with a
  key from mero-kms that keeps counters (`kms::KmsCounter`, `Config::rollback`), the
  node keeps an entry `(generation, mark)` in its store (`Engine::put_node_meta`, row
  `rollback`, bound to its node id) and raises it with mero-kms's every
  `CHECKPOINT_EVERY` (1 s), store first (`Checkpoints::run`, from `lib.rs`).
  `Node::start` judges the store before any group starts (`rollback::judge`): behind
  mero-kms, empty under a used id, mero-kms without the node's entry, another node's
  store, or a protected store answered without counters are refused; a KMS without
  counters and a store without one start unprotected. `Config::accept_rollback`
  (`merod run --accept-rollback`) resets the node's entry to the store's and raises
  the cluster epoch. Reported in `/ops/status` (`rollback`) and `/health`
  (`rollbackEpoch`). Tests: `tests/rollback.rs`, `rollback::tests` (`MemoryCounter`
  stands in for mero-kms).
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
  `maxFileBytesPerOrg` counts the plaintext bytes of a component's files.
  `SystemState::app_file_bytes` holds them per app, and `payer_file_bytes` is its sum per payer (`usage::payer`), kept in step by `set_app_file_bytes`.
  `SystemCmd::ChargeFileBytes { app_id, delta }` refuses a charge past the quota and floors a release at 0.
  `DeleteApp`, `DeleteOrg` and `DeleteTeam` free an app's files through `remove_app`.
  `TransferApp` of a personal app is refused when the new owner's quota would not hold its files.
  The exception is `maxFuelPerDeveloperPerDay` (Q20, D76, D78; `"0"`, the default,
  is no limit; a decimal string in JSON, a number refused, `system::decimal_u64`). It
  is charged to the app's developer, `usage::developer` (#4839), the same identity
  usage is billed to: the owner of the deployment an app was created from, else the
  app's organization or owner. So two payers' instances of one developer's
  deployment share one cap. The fuel lives in the app groups' usage books, so it is
  checked where a client's call, a delivered xcall or a synchronous xcall runs
  (`Node::fuel_quota_refusal`, from `call_local_component`, after the idempotent
  replay and the delivered-xcall check, before the run); an action's own calls are
  not refused, as its start was. The check asks no peer
  (`Node::developer_fuel_cached`): apps this node replicates are read from the
  replica, the others from `Node::refresh_usage`, which every `USAGE_REFRESH` (5 s)
  while a cap is set asks all peers at once (`internal/usage/today`) and keeps a
  silent peer's last figures; so the cap is approximate by one interval and an app
  no peer ever reported counts as nothing (fails open). A refused delivered xcall is
  the target's final answer (D29, `deliver_one`), not a dead letter. The refusal is
  not logged, so the key is not spent; `init`, timer ticks and event handlers are
  never refused, and they still count toward the developer's day. The quota is fuel;
  wall time is information only (D76). A non-zero cap is written only once every
  registered node's attested release is this node's or newer
  (`Node::nodes_behind_release`, as D71 gates `base_seq`), else `409 UPGRADE_PENDING`
  (`api::UPGRADE_PENDING`, one code for every such release gate), judged before the
  operator is asked to sign (`__set_quotas`, #4791): an older node would drop the
  field and its snapshots would reset the cap. Accepted gap (#4809): an older node
  added after the cap is set, then sent a quota change itself, writes the quotas
  without the cap; the runbook says not to add nodes older than `1.0.0-rc.12` then,
  and the W19 minimum-release gate closes it. Usage reads (`app_usage`,
  `internal/usage`, `replicas_usage_on`) use `replica_group()`, never a group the
  node keeps after the app moved away. Regressions:
  `the_fuel_cap_asks_no_peer_during_a_call_while_a_replica_hangs`,
  `an_over_cap_developer_is_refused_within_one_refresh_while_a_peer_hangs`,
  `a_delivered_xcall_into_an_over_cap_developers_app_is_refused`,
  `the_fuel_cap_is_a_decimal_string`, `a_fuel_cap_waits_until_every_node_runs_this_release`,
  and `deployments::two_payers_of_one_developers_apps_share_its_fuel_cap`.
- **Read-your-writes across nodes**: lookups of system objects (`Node::sys_until`) wait
  up to `Config::read_wait` (2 s) for the local replica to apply a write committed
  elsewhere (`sys_fresh` waits as long, and says whether it caught up). A
  refresh is judged by the system log, not by a stale replica: a token this
  replica cannot find, or whose exchange the log refuses, goes to the log as
  `RefreshReused`, which revokes the session if the token was exchanged before
  and is refused if it never was (then a replica that is behind answers 503,
  never "unknown"). Regression:
  `a_reused_refresh_token_revokes_the_session_on_a_lagging_node`.
- **Kept refresh tokens** (`api::kept_refresh`, `Config::refresh_rotate_after`,
  production `api::REFRESH_ROTATE_AFTER`, 1 h; the test harness 0): a refresh
  token younger than that mints a session from the system state at a read
  index (`sys_fresh`), live, its session not revoked, its passkey still the
  account's, and writes nothing; older, unkeyed, not caught up or not held
  fresh, it goes through `UseRefresh` as above. Every active session then
  writes once per rotation, not once per `SESSION_TTL`. Regressions:
  `tests/refresh.rs`.

## Component executor (`calimero:app` components, the v2 SDK)

Spec `app-runtime-v0.14`, on the node's log,
API and system group:

- **Engine and host** (`node/component.rs`): one `WasmtimeEngine` per node
  (`Node::app_engine`), compiled artifacts cached under `<data_dir>/components`, each
  with a MAC under a key derived from K_c (S8). `Node::prepared_for` prepares a bundle
  once (single-flight, Ruling 44; `concurrent_first_uses_of_a_bundle_prepare_it_once`).
  `POST /bundles` and package installs run the engine's `prepare` on a component
  (a core module is refused, see "SDK 0.12 apps").
- **State** (`component::ComponentState`, D46): the component's stored keys
  (generation prefix included, D39) are hidden rows of the group (`rows::CKV`, sealed
  and HMAC-keyed), and each replica keeps them in memory too as an
  ordered `Arc<BTreeMap>`: a run gets the current version as its `StateView` (ordered
  `range` scans, which hidden rows cannot answer); `apply` copies it only while a run
  still holds the previous one; `restore` (a restart, a snapshot install) reads it
  back from the rows. Host records (`ComponentHead`: `last_ts_ms`, `last_id`,
  `install_ts_ms`, `timers`, the idempotency rings) live in the group's `Head`.
  `last_write_ms` is the entry timestamp of the newest call that applied with a
  diff or files, or of an uploaded blob or file (`ComponentHead::note_write`;
  not `init`, ticks, refused calls or conflicts). It is left out at zero, so a head from before
  it reads and writes as it did
  (`a_head_from_before_the_last_write_reads_and_writes_as_it_did`).
  Idempotency (D74 in spec v0.26, `idempotency.rs`):
  records in `rows::CIDEM`, a ring per class in `rows::corder`. A key's class is its prefix (`class_of`): a `node:` key (the
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
  `a_callers_key_containing_node_is_kept_24_hours`,
  `records_over_the_new_cap_drain_a_bounded_number_per_entry`.
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
  copy every run uses); `base_index` is the app's last entry
  (`AppState::last_index`). Statuses (spec section 5, D23,
  D36): `422 APP_ERROR` with `details.{code, fields}`, `422 TRAP` with
  `details.{kind, detail}`, `503 TIMEOUT` (not logged, key not spent); arguments
  over `MAX_ARGS_BYTES` (512 KiB) are `413 ARGS_TOO_LARGE`, an entry over the log's
  cap is logged trapped `HostDenied` (D50). Answers carry the node's receipt.
- **ABI gate** (`Node::abi_gate`, `abi_check`, S5, Ruling 31/40): from the ABI of the
  bundle the group runs; default-deny; `/intents` reads `view` and `confirm` from it
  (`AppRecord::confirm_methods` is always empty now: a request names none). A compute method
  (kind `compute`) and a `computeResult` method are `WrongKind` on `/intents` and `/views` for
  every caller, System included: only the node runs them.
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
  send it, so a removed member stops receiving at once.
- **Usage** (Q20, `component::RunUsage`, `DayUsage`): each `componentInit`,
  `componentCall` and `componentTick` outcome carries `usage { fuel, wallMs, writes,
  bytesWritten, events }`, measured by the leader (`Outcome::from_host`; a `HostDenied`
  replacement keeps fuel, wall time and writes). Applying a non-conflicting entry adds
  it to `ComponentHead::usage` (a `UsageBook`), per UTC day of the entry's `ts_ms`,
  pruning `USAGE_DAYS` (31) back from the earlier of that day and the day after the
  newest one held, so a far-future stamp never erases the real days
  (`a_far_future_entry_does_not_erase_the_real_days`); followers never measure, so
  every replica's book is the same (a snapshot carries it in `Head`). Timeouts are
  not logged, so not counted (#4736); views are not counted (Q9); actions and compute
  runs are not in the book yet. The API gives every figure as a decimal string (D20):
  `/apps/{app}/usage`, `bundleBytes`, `developerFuelToday`, `developerWritesToday`
  in `/auth/usage`, and `developerFuelToday`, `developerWritesToday` in
  `/orgs/{org}/usage`. Tests: `the_usage_book_sums_what_the_entries_carry`,
  `usage_survives_a_leader_change_and_a_snapshot_install`,
  `an_account_over_its_daily_fuel_gets_429_and_its_timers_still_tick`,
  `a_fuel_limit_of_zero_never_refuses`, `only_the_owner_and_org_admins_read_an_apps_usage`,
  `an_org_apps_runs_count_toward_the_org_not_its_owner`,
  `an_org_over_its_fuel_cap_gets_429_while_its_owners_personal_apps_run`,
  `only_an_orgs_admins_read_its_usage`.
- **Upgrades**: the node's route and authority (`POST /apps/{app}/upgrade`,
  passkey consent, `UpgradeApp`); at the leader a component upgrades code-only
  (`componentUpgrade`) and a named `migrate` is refused `MigrationsNotSupported`.
- **Identity a component sees** (`component::SysDirectory`): its org, owner, the
  caller's role and teams there; `orgs-of` answers only the app's own organization
  (D75, spec v0.26): for an account, at most the app's org with its role, and nothing
  for a personal app. Test: `a_component_sees_no_organization_but_its_own`.
- **Per-executor endpoints** (`the_abi_storage_and_blob_routes_answer_for_a_component`):
  `/apps/{app}/abi` answers the ABI the component embeds (`calimero:abi`);
  `/apps/{app}/storage` counts its keys, bytes and blobs (see "Files" for the file routes). The host refuses event
  handlers and TEE runs to a component, and an xcall into a wasmer app is a dead letter.
- **Xcalls** (D13, D25, D29, `component/xcall.rs`): an `Ok` entry's `Outcome::xcalls`
  go to the source's outbox (`XcallHead` in `ComponentHead`, rows `rows::COUT ‖ seq`,
  at most `MAX_OUTBOX`). The source leader's timer poll (`Node::deliver_xcalls`, at most
  `DELIVER_BATCH` a round) delivers the head through `call_component` with
  `CallRequest::xcall = XcallFrom { source, seq }`, `Caller::App`, a `node:xcall-…` key;
  the target checks the boundary again (`same_boundary`: one org, or one owner's
  personal apps), its watermark (`XcallHead::seen`, D25: answered as delivered without
  running), the ABI (`allowApps`, `"*"` = any; `abi_check`), runs `kind = xcall-deliver`
  and records `CallEntry::xcall`, whose apply moves the watermark whatever the status.
  The source then logs `componentXcallDone { seq, dead }`. Timeouts and `5xx` are retried
  (`xcall_retry`, in memory, `backoff_ms`) until `XCALL_WINDOW_MS` from the send; a
  missing target, another org's, a wasmer app, or the window passing dead-letters it
  (`rows::CDEAD`, newest `MAX_DEAD_LETTERS`) and runs the source's `onXcallFailed` if its
  ABI allows the system. `GET /apps/{app}/xcalls` (owner or org admin, a hosting node).
  Test: `component_xcalls_are_delivered_in_order_or_dead_lettered`.
- **Blobs** (R8, `component/blobs.rs`): the cluster's store and routes
  (`store_blob`, a majority, `fetch_blob`, `collect_blob`, `blob_held_strong`), but the
  app holds a blob through a host record (`BlobRecord { size, mime, refs, idleSinceMs }`,
  hidden rows `rows::CBLOB`, in memory as `BlobTable`), not a mark in its keyspace.
  Upload (`POST /apps/{app}/blobs`, `Content-Type` kept as the mime, at most
  `MAX_BLOB_MIME_BYTES`) commits `componentBlob`; the owner's delete
  `componentBlobForget` (both through `CallRequest::host`, `Node::component_blob_cmd`).
  A `BlobRef` in a stored value is CBOR tag `calimero_app_host::BLOB_REF_TAG`; `apply`
  counts references from each diff and the values it replaces (`ref_deltas`), so every
  replica agrees; a blob with none is held `BLOB_IDLE_MS` (1 h) from its upload or its
  last reference, then forgotten against entry timestamps (`expiry_now`, at most
  `BLOB_EXPIRY_STEP` per entry) and queued in `ComponentHead::collect`; the leader's
  timer poll collects the bytes (`collect_blob`) and logs `componentBlobsCollected`.
  Runs see the records through `RunBlobs` (`HostCore::with_blobs`): `info`, and `grant`
  = `<expiresMs>.<HMAC>` under `grant_key(K_c)`, which `GET /apps/{app}/blobs/{id}?grant=`
  takes without a session (a bad one is `404`). At most `MAX_COMPONENT_BLOBS` per app
  (`429`). Tests: `a_components_blobs_are_held_by_references_and_granted`,
  `an_owner_deletes_a_components_blob`.
- **Search** (spec section 7, `component/search.rs`, `Node::component_search`): the ABI's
  `state.search` names each searchable table's text fields; every record
  (`generation ‖ (table, "r", key..)`, `record_table`) is a `calimero-search` document
  (id = SHA-256 of the stored key). A view brings the app's indexes up to the state it
  reads before it runs (`ComponentSearch::for_view`, inside `read_local`): from the record
  keys the entries since touched (`AppMachine::note_touched` at apply, the same
  `TOUCHED_RING`), else a full scan; a new bundle rebuilds. The run
  gets it through `HostCore::with_search` (`RunSearch`): hits are the guest's record keys.
  Derived, in memory, dropped with the app group. Test:
  `views_search_a_components_tables_as_they_change`.
- **Actions** (D12, D17, D22, D27, S9, R14; `component/action.rs`): a client's call of an
  `#[app::action]` method (`abi_check` lets an account or API key start one) logs an `Ok`
  call whose outcome carries `ActionOut { id: a.<term>.<seq>.0 }` and answers
  `{ actionId }`; `start-action` in a call or timer does the same from the run. Apply
  records it pending (`rows::CACT ‖ id`, `ActionsHead::pending`, at most
  `MAX_PENDING_ACTIONS`) with the starter's `Caller` and the entry time. The leader's
  timer poll (`Node::run_actions`) runs pending ones it is not running in a pool of
  `MAX_ACTION_WORKERS` (`action_slots`): `attempt_action` on a blocking thread, `kind =
  action`, `Budget::action()` (10 s), the starter as caller, monotonic clock at the start;
  `NodeAction` serves `view` (`Node::view`, strong) and `call` (`call_component` with the
  D27 key and `CallRequest::action`, which alone reaches `allow(action)` methods), run on
  the runtime while the action's thread waits; `NodeHttp` sends through `outbound_for`
  (the signed release's prefixes, the owner's approval, the operator's hosts,
  `{{secret:..}}`, one request per run). The end is logged as `componentActionEnd`: a
  timeout retries (`action_retry`, `retry_after_ms`) until `MAX_ATTEMPTS`, an app error,
  trap or removed method fails at once; a failure sends `ActionFailed { actionId,
  method, reason }` to the starter (an event with an `Accounts` audience) and runs
  `onActionFailed` as the starter. A new leader resumes what is pending; an end that
  another leader logged first changes nothing. Test:
  `actions_run_on_the_leader_and_reach_only_declared_approved_hosts`.
- **Outbound HTTP from calls** (D76; `RunExtras::outbound`, `TimedHttp`): calls (but a
  read-only member's), timers and deliveries get `NodeHttp` over `outbound_for`, as actions
  do, each response awaited at most `CALL_FETCH_TIMEOUT` (1.5 s, inside the 2 s budget;
  then the app's fetch gets an error). The host core records each exchange
  (`Outcome::http`, `component::HttpRecord`: the request with its placeholders, the
  redacted response, bodies cut to 64 KiB with their lengths) in the call's entry,
  whatever its status; followers never fetch, and a keyed retry replays. Tests:
  `a_call_checks_a_turnstile_token_through_a_captcha_service`,
  `a_runs_outbound_exchange_is_recorded_in_its_entry`.
- **Synchronous xcalls** (`NodeXcall`): a run made with a key (`RunExtras::key`: a
  call's `call:<caller>:<idempotency key>`, a tick's `tick:<method>:<slot>`, an action's
  `action:<id>`, a view's placeholder) reaches other apps through `xcall.call`/`xcall.view`.
  The target must be another component app within `same_boundary` (any other answer is
  one `no such app in this organization`). A call goes through `call_component` with the
  host core's derived key, `Caller::App(source)` and `CallRequest::sync_from` (the target
  checks the boundary again and runs it as `xcall-deliver`, so it reaches no further); a
  view through `Node::view` (strong). The run's thread waits at most `SYNC_XCALL_WAIT`
  (3 s) while the runtime runs it; two apps calling each other at once both time out.
  Test: `a_call_verifies_a_captcha_with_another_app_in_the_same_request`
  (`apps/v2-fixtures/captcha`).
- **Files** (`node/component/files.rs`; tests `component_files.rs`): a file is a sealed blob.
  It shares the blob id namespace and the `BlobRef` type; there is no delete, retention is R8.
  A run's `files` provider (`NodeFiles`) is built per run from the app id, the file key (`backup::file_key`, not the TEE key) and the app's blob records.
  A sealed file's record carries a `FileSeal` (`BlobRecord::file`: the plaintext hash and the wrapped key) in the hidden `rows::CBLOB` rows the guest cannot reach, so a snapshot and `restore` carry it.
  A writer seals (`SegmentWriter`), stores each segment on this node the moment it is complete and wraps the data key; the file id is the hash of its segment list (`FileSeal::segments`, `FileMark::segments`).
  Each segment is stored in the `blobs` column under `seg.<hex hash>` and, in the same batch, an empty index row `seg/<hash>` in `meta` (`Engine::put_segment`, `list_segments`), so segments are listed without reading a byte of them; no whole-file row exists.
  Segments travel raw (no base64): a push is `PUT /internal/segment/{hash}`, checked on receipt (`SHA-256(body)` is the hash, at most `SEGMENT_BYTES`), a fetch `GET /internal/segment/{hash}`, served only to a node approved for an app hosted here whose files list it (`caller_may_hold`; `404` for a hash no hosted app lists); there is no peer delete, a replica's unreferenced segments go with the sweep after `SEGMENT_GRACE` (`Node::pin_segments` holds an upload's or a pending entry's out of it).
  A job transfer still sends and receives the joined segments in one request.
  The outcome and the entry carry marks only (`Outcome::files`, a serde default: old entries decode unchanged).
  `apply` records them as unreferenced blobs before the diff, only for status `Ok`, so a `BlobRef` the same entry stores is counted and a file is never collectable in between.
  A file is then held while a stored `BlobRef` names it, `BLOB_IDLE_MS` after its entry or after the last reference went, and forgotten by `expire_blobs` in the same entry on every replica.
  The leader then collects the file's id through the same `collect_blob` and `componentBlobsCollected` as any blob, which releases its quota and deletes no file bytes: no blob path (`collect_blob`, the blob routes) deletes a `seg.` row.
  Every node deletes segments itself: `Node::sweep_segments` (every `SWEEP_EVERY`, 10 min, over the `meta` index; `node/segments.rs`) deletes a stored segment no hosted app lists (`BlobTable::holds_segment`, a map from segment hash to file rebuilt on `load`) and nothing pins, if this node saw it listed at an earlier sweep (its file was forgotten) or it has been unlisted for `SEGMENT_GRACE` (24 h) since first seen so; first-seen times are in memory, so a restart restarts the grace, and there is no caught-up check, so segments handed to a joining node before it applies survive the grace.
  `BlobTable::upload` and `forget_blob` refuse a file id, so a blob upload or an owner delete cannot take one.
  **Publishing** (`runs.publish`): `NodeFiles::publish` answers only a sealed file whose `FileSeal.output` is set, which `apply_compute_final` sets on a done run's outputs and nothing else sets.
  It opens and proves the whole file, holds the plaintext against the pending budget (`plaintext`), and answers a `PublishedFile` (a file over `MAX_BLOB_BYTES`, the plain blob it becomes, is a soft `file too large`).
  The outcome and entry carry `Outcome::published` (`PublishedMark { source, id, size }`, never bytes; a serde default).
  `run_with` adds the plaintext of each published blob the app does not hold to the run's blobs, so `logged_entry` charges it (`new_file_bytes`, once per id) and `persist_files` pushes it to a majority before the entry is proposed.
  `apply_component_outcome` checks every mark first (`publishes_only_outputs`): the source must be an output record, its plaintext hash must be the mark's id and its size the mark's, and at most `MAX_PUBLISHED_PER_RUN` marks.
  A failing mark makes the whole entry a no-op that answers `NOT_A_RUN_OUTPUT`, on every replica, so a leader cannot publish an uploaded file, a file a call made, another app's output or a different plaintext.
  Any entry that applies as a no-op (a conflict or a refusal) has the bytes it stored discarded and its charge refunded (`recorded_nothing`).
  Then `BlobTable::add_published` records the plain blob (`BlobRecord::charged`, `application/octet-stream`) before the diff, unless the app already holds that id; `expire` and `BlobsCollected` release the charge of a file or a charged blob.
  A published blob is served only through a grant (`blob_download` judges it on the leader's record, `Node::blob_standing_strong`), and `blobs.grant` is refused for it outside a logged write.
  `BlobRecord::epoch` counts the owner's revokes (`ComponentCmd::BlobRevoke`, `POST /apps/{app}/blobs/{id}/revoke`, `__blob_revoke`); a grant above zero carries its epoch (`grant_token`), and a download whose grant names another epoch than the record's answers `404`.
  The app also keeps an epoch floor (`BlobTable::floor`, the hidden `rows::CBLOB` row `floor`): forgetting any blob but a sealed file raises it above that blob's epoch, and a record made again (`add_published`, `upload`) starts there, so a grant of a forgotten blob never holds for its successor.
  `load` fails on an unreadable floor or record instead of defaulting, and `revoke` refuses at `u32::MAX` (`no revocation left`).
  The owner's delete (`forget_blob`) forgets a published blob too, queuing its bytes and charge in `collect` and `collect_files` like `expire` does; it still refuses a sealed file.
  A result method sees its own run's outputs without the flag (`blob_view_with_outputs`), so it cannot publish them; it stores the `BlobRef` and a later call publishes.
  Tests: `component/tests/runs.rs` (the apply rule) and `compute_runs.rs` (`a_published_output_downloads_as_exactly_the_plaintext_the_run_wrote`, `only_an_output_of_this_apps_done_runs_publishes`, `a_published_blob_survives_a_leader_change`).
  `RunBlobs::info` answers a file with its plaintext size and hash and an empty mime, as `files.info` does; the host core refuses `blobs.grant` for it.
  `GET`, `POST` and `DELETE` on `/apps/{app}/blobs` refuse a file id with `409 NOT_SUPPORTED` (`refuse_sealed_file`, judged by the app's leader through `file_held_strong`).
  `/apps/{app}/storage` counts their sealed bytes in `blobBytes` (a tag per chunk).
  **Uploads** are `POST /apps/{app}/files` for a file of up to one part (`PART_BYTES`, 16 MiB; `Client::upload_file`) and, for a larger one, upload sessions (`node/component/uploads.rs`): `POST .../files/uploads { size }` answers `{ uploadId, partBytes, parts, expiresAt }` (Unix seconds), `PUT .../uploads/{u}/parts/{k}` takes part `k` raw and answers `{ next }`, `GET .../uploads/{u}` answers `next`, `POST .../uploads/{u}/complete` waits up to `COMPLETE_WAIT` (20 s) for every segment to reach a majority (else `503 FILES_UNAVAILABLE`; a retry resumes) and answers the file, `DELETE .../uploads/{u}` aborts.
  Only the app's owner and the members of its organization (role Member or above, `api::may_handle_files`, through `api::files_gate`) may upload, list and read files, even where the app is public; anyone else gets `403` on every `/files` route.
  `GET /apps/{app}/files` lists the files (`{ files: [{ id, blobId, size, sha256 }] }`) and `GET /apps/{app}/files/{id}` answers one (never the key); `/apps/{app}/blobs` lists plain blobs only and answers `409 NOT_SUPPORTED` to `info` of a file id, as to a download.
  A session lives in the leader's memory (`Uploads`, one `Session` per upload: segment writer, segment and part hashes, what is on a majority), belongs to its account (another account, an unknown id, a leader change, `UPLOAD_IDLE` 15 minutes without a part or `UPLOAD_TTL` 6 hours all answer `404 UPLOAD_UNKNOWN`) and is ended by `Node::expire_uploads` (the timer poll) when it lapses or this node stops leading the app.
  Part `k` is sealed into segment `k` (`SegmentWriter::write_more`: a full part closes its segment without waiting for the next) and stored here, and a background task pushes it to a majority (`push_blobs`, no discard on failure; retried with backoff until it lands or the session ends); the answer waits only for a slot in the window (at most `UPLOAD_WINDOW` = 4 segments stored but not yet on a majority; the part is sealed and stored anyway, and past `PART_PUSH_WAIT` the answer is `503 FILES_UNAVAILABLE` and the resent part replays its answer); parts go in order (`409 PART_OUT_OF_ORDER` names `next`), a resent part answers its first answer and other bytes are `409 PART_CONFLICT`, a part of the wrong length `400 BAD_PART`.
  Begin refuses past 4 sessions per app, 32 per node or 16 GiB declared per leader (`429 TOO_MANY_UPLOADS`, `admit`) and when the payer's file bytes plus its open sessions plus `size` pass the quota (`429 QUOTA_EXCEEDED`); nothing is charged until `complete`, which charges once (`State::charged`) and commits through `commit_mark` under the key `node:upload-<id>`, so a retried `complete` replays, from the leader's record of that key (`replayed`, only for the uploader) even when no session is left.
  A `complete` before every part is in is `409 UPLOAD_INCOMPLETE`; one past the quota is `429`, deletes the bytes and ends the session; one whose commit may yet land (`Upload::Unknown`) keeps the session and its bytes until it ends, when a barrier tells whether the mark committed: absent, the charge is refunded and the segments deleted (`Node::undo_unmarked`).
  Ending a session (abort, idle, age, leader change) answers unknown at once, then stops its pushes and deletes its segments here (`Node::end`); replicas' copies go with the sweep.
  A follower forwards every session request one hop to the leader (`internal/upload*`, raw part bodies, `PART_FORWARD_TIMEOUT`).
  The single request (`POST /apps/{app}/files`, one part) is a session of one part run through the same operations (`Node::upload_file`): begin, part 0, complete, retrying a `503` `complete`, starting over when the leader changed, aborting on failure.
  Tests: `component_files.rs` (`a_three_part_upload_commits_one_mark_at_one_index` and the session tests after it).
  `StorageStats::files` carries each file's id, size and SHA-256 for these routes, and `internal/storage` sends it to a peer.
  The command carries the uploader (`uploader`, a serde default), the account the API's `principal` derived, which the `internal/upload*` routes refuse unless it is an account id (`valid_uploader`); `BlobTable::add_uploaded` stores it as `FileSeal::uploader`, and files made by calls and runs have none.
  `NodeFiles::uploader` answers it to `runs.uploader`; an id already held is refused (`file exists`), so a second upload never replaces the first uploader.
  A recorded answer for another file under the same key wins and the new file is undone.
  `apply` records the file as an unreferenced blob stamped with the entry's `ts_ms`, as `Node::component_blob_cmd` stamps an upload, so a client has the idle time to attach it.
  If the commit fails, a `componentBarrier` entry (applies as nothing) is committed through the leader, so the failed entry has either committed or never will.
  The leader then reads the record: present, the upload succeeded; absent, its bytes and charge go; no barrier possible, they stay.
  A file is held while its blob record exists, and the quota counts it for as long.
  A logged entry's new files are charged first, within `CHARGE_WAIT` (`Node::charge_files`, from the run's thread).
  A refused charge pushes nothing, answers `429` and logs nothing; a charge that lands after the wait is refunded when its answer arrives.
  `Node::discard` undoes the bytes and the charge if the entry does not commit.
  The charge is released by retention: `expire_blobs` queues a forgotten file's plaintext size in `ComponentHead::collect_files` beside its id in `collect`.
  The leader's `componentBlobsCollected` entry answers `freed`, the bytes of the queued files it really took off, and the node releases that many (`collect_component_blobs`), so a repeated entry releases nothing.
  A node that stops hosting an app that is deleted everywhere drops its sealed files' segments (`Node::drop_deleted_files`); plain blobs stay, as they may be another app's.
  A crash between the entry and the release leaves an over-count, never a loss; a queue past `MAX_COLLECT_QUEUE` drops its oldest ids with their sizes.
  Before a call, tick or `init` proposes, `Node::persist_files` pushes the new files' segments, read back from this node's store, at most `MAX_PUSHING_BLOBS` (2) at a time and each body shared by every target, to the app group's voters and learners (its own Raft membership, not the directory's replicas), under one deadline below `FORWARD_TIMEOUT` and a node-wide byte budget (`MAX_PUSH_BYTES`).
  It needs a majority of the voters, this node counted; a voter with no address still counts.
  It runs only after the entry bytes are built, so an entry the log replaces as too large stores nothing.
  When the majority is missing the call is `503 FILES_UNAVAILABLE` (the forward loops pass it on unchanged and do not retry it; the client does, with its key), nothing is logged and the pushed bytes are removed; so are the bytes of an entry that conflicts or whose proposal fails.
  A run's writers hold only their partial segment in memory, which draws on a node-wide budget (`MAX_PENDING_FILE_BYTES`, `PendingBytes`) and returns as each segment is stored; the segments of a file no entry marks are deleted when the run ends (`NodeFiles::forget_made`).
  A write it cannot hold takes nothing and aborts the logged run as `FileError::Unavailable`, so runs together never hold more than the budget.
  `open` reads the file one segment at a time: a local segment is used only if its SHA-256 is its hash, else it is dropped and fetched from the other replicas (5 s); a segment nobody holds ends the read as unavailable.
  Any opener failure, including at the end of the stream, is `ReadError::Corrupt`, which the host core makes a `HostDenied` trap.
  Open readers hold one segment and its plaintext (`OPEN_READER_BYTES`, at most the file) against a per-run budget (`MAX_RUN_OPEN_BYTES`) and a node-wide one (`MAX_OPEN_FILE_BYTES`; logged runs wait 1 s for it).
  Only a compute run opens a file (the host core refuses `files.open` in every other kind), so a sealed file's contents are readable only inside a compute run of the release the organization approved.
  A file that cannot be had (no replica holds it, or a budget is spent) aborts a logged run as `503 TIMEOUT`, not logged, key not spent (`FileError::Unavailable`, for `create`, `write` and `finish`) and ends a compute run `Unavailable`.
  Tests read a file through `Node::read_sealed_file` (feature `testing`), which uses the provider a run reads with.
  The provider is built for every run but touches keys, budgets and bytes only when a run touches a file.
  Blocking on the runtime from a run needs a multi-thread runtime, else the run is refused.
  Pushes skip members `may_send` refuses, and `fetch_blob_from` asks only the app's other replicas, which serve a file as a blob their app holds (`holds_blob`); bundles and other blobs still use every node.
  Replica moves hand over the app's files, segment by segment, with the bundle (`hand_over_bytes`), and again after the voters change, only what the joining nodes do not hold, for files made while the move ran; the second hand-over is retried for 10 s and never abandons the move.
  **Upgrade note (quota):** an older system leader rejects `ChargeFileBytes`, so uploads and calls that create files fail until the system group runs this release.
  A snapshot from before carries no file-byte counts, so files stored earlier start from zero, and their release floors at zero.
  **Upgrade note (rolling upgrade):** a node on a release without files refuses to prepare a bundle that imports `calimero:app/files` (`ForbiddenImport`), so it never runs, leads or answers for an app that uses files.
  Replicas of different releases coexist only while a release upgrade moves apps, and the old ones are retired.
- **Not here**: the spec's `__upgrade` through `/intents` (Ruling 36; the node refuses
  every `__` method there) and the leader's own re-check of upgrade authority
  (Ruling 45; the node judges it in the API and the system log); their tests are
  `#[ignore]`d with the reason.

## Not done yet

keys per app from mero-kms (phase 1 of `docs/specs/rollback-protection.md`), the
rollback epoch in the attested KMS evidence, snapshot restore into a
running cluster, event audiences, search indexes that survive a restart (rebuilt from state now, components' too).

## Commands

```bash
cargo test -p calimero-node    # 3-node cluster in process, sealed, real WASM
# component fixtures (apps/v2-fixtures/{kv,org-gated}, apps/kv-store-with-shared-storage,
# apps/rooms, any app a scenario installs) are built on first use (harness/cluster.rs::fixture,
# tests/common/mod.rs) into $CARGO_TARGET_DIR/app-components (needs the wasm32-wasip2 target)
cargo test --release -p calimero-node --test rooms   # Rooms A1 (shared scenario), A3, A5
# every scenario file on an in-process cluster (runner: tests/harness/scenario.rs): apps/*/scenarios/*.json
# (+ <name>.org.json), apps/*/scenarios/cluster/**/*.json, workflows/**/scenarios/**/*.json;
# tests/scenarios-skip.txt lists the ones reported, not run (docs: contribute/testing#scenarios)
cargo test -p calimero-node --test scenarios [-- FILTER]   # SCENARIO_FILTER, SCENARIO_SHARD=k/n
# A4, the fault test (merod run --dev processes on ports 7600-7699, shifted by
# WORKFLOW_PORT_OFFSET when set; release merod and
# meroctl in RV2_BIN_DIR, default target/release; the cached Rooms component)
python3 -I crates/node/tests/fault/rooms_fault.py --iterations 200 --seed 1
# the process suite (tests/fault/README.md): crash/freeze/restart, frozen leader, orgs and
# keys, signed packages, rolling upgrade under load, the kv-store soak; the kv component
# (target/app-components/kv.wasm, mero-app-build) and dist/com.calimero.kv-store-{1.0.0,1.1.0}.mpk
# (scripts/pack-component.py)
python3 -I crates/node/tests/fault/suite.py [--only NAME,..] [--iterations N] [--nightly]
python3 -I crates/node/tests/fault/suite.py --smoke --merod PATH   # the release image's set
python3 -I crates/node/tests/fault/suite.py --only kv-soak --soak-report soak.json   # soak.py report renders it
# compute actions end to end: 3 replicas and 2 job nodes as processes, the fixture installed as a signed
# .mpk with app and compute components, then apps/compute-redact end to end (same binaries plus
# mero-sign, ports 7650-7655; mero-app-build and mero-compute on PATH)
python3 -I crates/node/tests/fault/compute_e2e.py
# calls/s and p50/p99 through that cluster (harness: tests/harness/cluster.rs), as JSON:
# (also session refreshes and the system log's entries/s they cost, exchanged and kept)
cargo bench -p calimero-node --bench cluster_throughput -- --secs 15 --out cluster.json
cargo run -p merod -- --help
cargo run -p merod -- local   # dev cluster: 3 nodes on :2501-2503 (`local.rs`)
cargo run -p merod -- --node n1 inspect [--storage-key-file F]   # a stopped node's store (`inspect.rs`)
```
