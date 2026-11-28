# meroctl

The one command line for Calimero: making an app and shipping it (`app new`,
`build`, `codegen`, `abi`, `signer`, `pack`, `sign`, `inspect`, `publish`; no node,
`make.rs`), a development cluster and the loop around it (`local`, `local.rs`),
Calimero Cloud (`cloud`, `cloud.rs`), and one command tree over a node's API
through `calimero-client` (every request attested and sealed; the node's quote
is judged before anything is sent). Each command
calls the client's generated route methods (`client.orgs().invite_create(..)`,
from the node's OpenAPI document; `crates/client/AGENTS.md`) and prints the
typed answer serialized back to JSON; `raw` and `proxy` send any route through
`Transport`.

Names (commands, flags, value names, help, output keys) are the glossary's:
`package`, `instance`, `org`, `team`, never v1's application, context,
namespace or group (test `help_uses_the_glossary_words`; `scripts/check-vocabulary.py`
holds the API document and the docs to the same words).

| v1 | here | API |
| --- | --- | --- |
| `node add/remove/use/list` | saved nodes, `{home}/meroctl/nodes.json` | (local) |
| `app install/list/get/versions/get-latest-version/pipelines/uninstall/watch` → `package ...` (the `app` spellings still parse, hidden from `app`'s help: `mut_subcommand(.., hidden)` in `main.rs`; test `package_commands_are_packages_and_hidden_under_app`) | signed packages installed on the cluster (v1's applications); `--path` a `.mpk`, or a dev `.wasm` bundle; `package origins list/add/remove` its registered web origins (its manager's; `add --served` a custom domain the cluster serves the frontend on) | `/packages*`, `/bundles`, `/package-origins/{package}` |
| `instance create/list/get/delete/update/watch/alias/use` | app instances; `use` is the alias `default`; also `access`, `transfer`, `metadata`, `storage`, `abi`, `presence` | `/apps*`, `/auth/aliases*` |
| `call` | `call --instance I METHOD --args JSON [--view]` | `/apps/{app}/intents`, `/views` |
| `blob list/upload/download/info/delete` | an instance's blobs | `/apps/{app}/blobs*` |
| `file upload FILE -i INSTANCE` | an instance's sealed files: one request up to 16 MiB, else resumable parts (`Client::upload_file`) | `/apps/{app}/files*` |
| `file download INSTANCE ID -o OUT` | a sealed file's plaintext, streamed to `OUT.part` and renamed onto `OUT` only after every check passed (`Client::export_file_to`); `pipeline download` does the same per output | `/apps/{app}/files/{id}/export`, `.../segments/{k}` |
| `run logs APP RUN [--attempt N]`, `pipeline logs APP RUN --step S [--index I]` | print a compute run's module log (stdout, stderr and `log` lines) read through the files export; the latest attempt unless `--attempt`; a pipeline step's run is its `--index`th. A cluster keeps no run log, so against a current one it fails saying so and pointing at `local compute run` (`run::NO_RUN_LOGS`) | `/apps/{app}/runs/{run}/logs`, `/apps/{app}/files/{id}/export` |
| (new) `pipeline apply/validate/estimate/run/retry/get/cancel/watch/runs/receipt/logs/download` | an instance's pipeline runs: ship a pipeline file as a release of its package (`apply`: checked first, then `app release --pipeline FILE`, answering the registered pipelines with their pinned modules), check a pipeline source, estimate, start, follow (`watch`, SSE), the signed receipt (`receipt --verify`) | `/pipelines/validate`, `/packages*`, `/apps/{app}/pipelines/*`, `/apps/{app}/pipeline-runs*` |
| `org ...` | organizations: members, invites, teams, metadata, capabilities, outbound and release approvals | `/orgs*`, `/invites/*` |
| `team ...` | teams: members, metadata, reparent, subteams, settings; `team upgrade trigger/status/retry` (org-wide runs) | `/orgs/{org}/teams*`, `/orgs/{org}/upgrade*` |
| `account ...` | the account: devices (passkeys), API keys, usage, approvals | `/auth/*` |
| (new) `deployment list/get/create/delete/transfer/accept/cancel-transfer/offer/withdraw/secret list\|set\|clear/orgs/usage` | a developer's deployments (`deployment.rs`); `transfer --to` offers one to another owner, `accept --to` takes one offered to an owner this account acts for (`list` shows them under `offered`): a `DEPLOYMENT` argument is an id or the name of one the account manages (`resolve`, a shared name refused with the ids); `offer --package` defaults to the package the app already offers; `secret set` reads the value from stdin or a no-echo prompt (`read_secret`, `NoEcho`), never an argument; `usage --by org [--csv]` is `/orgs`' per-customer counts (the node meters per developer and today only, so `--since` is refused as not supported) | `/deployments*` |
| (new) `app release [DIR] --version V` | `pack` (its flags, `ReleaseArgs::pack_args`; DIR `.`), `publish` with `--publish`, then install (for `--install-org`, as `--org` is the Cloud organization `publish` takes): from the cluster's registry after `--publish`, else an upload; refuses a version not semver or not newer than the cluster's releases of the package (`make::check_newer`) before building. Needs a node | `/packages/{package}`, `/packages`, `/packages/install` |
| `dev start/watch` | install, create, keep pushing a build | |
| (new) `app new/build/codegen/abi show\|diff/uses update\|check/signer new\|id/pack/sign/inspect/publish` | no node: start from `calimero-network/apps` (`dev.rs::new`, a sparse clone, plus an `AGENTS.md` for coding agents (`agents_md`) and a `CLAUDE.md` importing it; test `dev_new_starts_a_project_from_an_app_of_the_apps_repo`), `calimero-app-build`'s builder and `abi_tools`, `uses update NAME --from <crate\|.mpk\|.wasm\|abi.json>` (`make/uses.rs`: checks the package, the semver range and the listed methods, then `calimero_app_build::uses::vendor` writes `calimero.lock` and `calimero/abi/NAME.json`) and `uses check` (`uses::check` plus the range), `meroctl-sign`, `calimero-bundle` (the frontend checked with `validate` and `check_frontend_html`), `pack --service NAME=PATH` (or the crate's `services`) writes a multi-service manifest, `pack` merges the `outbound` and `secrets` of each dependency the crate's `connectors` lists (`make/connectors.rs`, over `cargo metadata`), and `pack` opens what it wrote with `VerifiedBundle` before writing it; the registry's `POST /api/bundles/push` (`--org`: the Cloud's `POST /api/registry/token` first) | registry, Cloud |
| (new) `app check [DIR\|--mpk F] --against VERSION\|F.mpk [--fail-on red\|egress] [--allow-egress P]...` | `app/check.rs`: the candidate (packed with `app pack`'s flags, or `--mpk`) against the release it replaces, both as units (`uses` mode `create`, at most 3 deep, members from `--uses-mpk` then the node), judged by `calimero_bundle::update::classify`; the summary reports only destinations (`Data: stays in Calimero` or `Data can go to: <destinations>`; an external endpoint, a TEE included, is listed like any other) or `--json`; exit 0 judged, 1 the gate failed, 2 could not judge. Offline with `--against FILE.mpk` (`AppCmd::is_offline`). Tests: `tests/app_check.rs` | `/packages/{p}/{v}/capabilities`, `/packages/{p}` |
| (new) `local up/down/status/dev/test/compute run` | `merod local` (the release's image by default, `docker_ready` checking Docker first; `--local` or `MEROD`: the binary; `--registry-url`, default the public registry) in the background, state in `--dir`/`meroctl-local.json`, saved as node `local`; `dev` builds, uploads a raw bundle as a fresh deployer, writes `app/.env.development.local`, runs `pnpm dev` on :5173 and rebuilds on save; `compute run` is `mero-compute` (feature `compute`, default on) | `/bundles` |
| (new) `registry login/logout/list/search/versions/get` | the app registry's `GET /api/bundles` (`registry.rs`, no node): `search` filters the listing here (the registry has no search route); a key (`--api-key`, `CALIMERO_REGISTRY_API_KEY`, else the one `login` checked and saved per registry URL in `{home}/meroctl/registry.json`, 0600) lists private packages (`include_private=1`) and `--mine`; `app publish`, `app release --publish` (which refuses a version the registry already lists before building) and a version-less `package install PACKAGE` (the registry's latest) use it too. Tests: `tests/registry.rs` | registry |
| (new) `cloud login/logout/orgs/clusters list\|create/use`, `cloud compute pipeline\|logs` | the Cloud manager (mdma) with a console session in `{home}/meroctl/cloud.json` (refreshed from `X-MDMA-Session-Refresh`); `use` saves a cluster's node as the default | `/api/orgs`, `/api/clusters` |
| `network status` | `/health` | |
| (new) `login`, `logout [--all]`, `whoami`, `attest [node\|app INSTANCE]` | file passkeys on dev clusters; cluster verification | `/auth/*`, `/attestation` |
| (new) `attest release [--file F --nonce N]` | verify a `merod attest` release attestation (no session, no sealed channel) | `/attestation/release` |
| (new) `proxy [--listen ADDR] [--allow-remote] [--openapi FILE]` | the API as plain HTTP on loopback, each request through the attested, sealed `Transport` (`proxy.rs`) | every route; `GET /openapi.json` |
| (new) `ops ...` | the operator, `--ops-token`: status, cluster, metrics, placement, add-node, remove-node, move, backup, restore, backup-key, recovery-keygen, recover, pools/set-pools, prices/set-prices, quotas/set-quotas, release, outbound, packages/install/uninstall, upgrade, upgrade-org | `/ops/*` |

Dropped, with no meaning here: `context sync`, `group sync` (Raft replicates),
`context identity`, `account pair-*`/`seal-to`/`device-init` (a passkey is the
device), `peers` (no gossip), `tee fleet-join` (`ops add-node`).

- The registry and the Cloud's manager are called at `/api/...` only, never
  `/api/v2/...`. Tests: `publish_pushes_the_bundle_to_the_registry` (a
  service serving only `/api/v2` is refused with its `404`),
  `cloud_signs_in_lists_and_uses_a_cluster`.
- Sessions are kept per node URL in `{home}/meroctl/sessions.json` (0600) and
  refreshed when they expire. An instance argument is an alias of the account or
  an id (`Env::app`).
- What an owner approves (`instance update`, `instance transfer`, `org delete`, approvals,
  update policies (`org|instance upgrade-policy` with a flag), `instance approve-update`,
  `instance undo --yes`,
  `team upgrade trigger/retry`, `package origins add/remove`, a deployment's changes) takes `--dev-passkey FILE` on a development
  cluster: `DevPasskey` loads it, gives it to the generated method as its
  `passkey` (the routes the document marks `x-calimero-step-up`) and saves it
  after; without it the request is sent as is and the node answers `412
  CONFIRM_REQUIRED`: on a real cluster the owner approves in the console. `raw`
  does the same for any route (`Env::raw`, `Transport::confirmed`).
- An `ops` decision the document marks `x-calimero-operator` is signed with
  `--operator-key-file` (the generated method's `operator`); without the key
  the node's `428 OPERATOR_SIGNATURE_REQUIRED` comes back with a hint to pass it.
- `proxy` (`proxy.rs`) holds the user's credential for every local caller, so
  it binds to loopback (`--allow-remote` lifts that and the `Host` check, with
  a warning), refuses a non-loopback `Host` (DNS rebinding) and any request a
  browser sends for a page (`Origin`, `Sec-Fetch-Site`), attests the node
  before serving, passes a request's own `Authorization` through unrefreshed,
  and logs method, path and status only (never a header, query or body). Test:
  `the_proxy_serves_the_api_as_plain_http` in `tests/cli.rs`.
- Output is JSON: the generated method's typed answer serialized back
  (`env::to_json`); a field the client's types do not name is kept (`extra`),
  so nothing the node sent is dropped. JSON a command takes for a closed body
  (`ops set-quotas`) is refused on a key the body does not name, before it is
  sent. Test: `a_misspelled_quota_is_refused_before_it_is_sent`. Commands that stream (watch, presence
  stream, metrics, downloads) write their own and answer `null`, which is not
  printed (so does `deployment usage --csv`). Tests: `deployments` in
  `tests/cli.rs` (release, offer, a secret from stdin, a customer counted,
  take-down), `deployment_and_release_commands_parse`.

- `main` runs the commands that need no node (`node`, `local`, `registry`, `cloud` but
  `cloud compute`, `app`'s offline ones: `AppCmd::is_offline`) before it
  resolves a node, so they never read `--node`/`--api`. The old spellings
  (`dev new`, `pipeline`, `run logs`, `attest-release`, `attest --context`, `context` (now `instance`) and `--context`/`-c` (now `--instance`/`-i`),
  `namespace`/`ns` (now `org`, with `teams`, `create-team`), `group` (now `team`, with `subteams`),
  `--namespace`/`--group` (now `--org`/`--team`) and `ops upgrade-namespace` (now `upgrade-org`))
  warned in 1.0.0-rc.30 and are refused since; a refused command line that used one gets a
  note naming its replacement under clap's error (`old_spellings`, tests
  `old_spellings_are_refused_and_named`, `old_spellings_are_named_with_their_replacements`);
  `the_current_spellings_parse` and
  `the_command_tree_is_consistent` (clap's `debug_assert`) pin the tree, and
  `help_says_instance_org_and_team` keeps every visible help saying instance, org and team.
- `local up` without ownership leaves the cluster running; `local dev` owns
  what it started (`Started`, stopped on drop). Readiness is every port
  listening and `merod local`'s `ops token` line in the log.
- `local up` runs one node unless `--nodes` says otherwise (core's own tests
  run three). `cluster.json` in the state directory keeps what a cluster was
  made with (its node count, and its Docker volume, which is also the
  container's name), so the next `up` starts the same cluster: a different
  `--nodes` without `--fresh` is refused, since a cluster restarted with fewer
  nodes never reaches a majority. The volume is named after the state
  directory (`volume_of`), never the port alone, so two directories on one port
  never share data. A cluster an earlier meroctl made (no `cluster.json`, a
  `merod.log`) keeps 3 nodes and the port's volume (`made`, `plan`); `up` writes
  `cluster.json` before it creates `merod.log`, so an `up` that failed to start is
  never read as such a cluster (`an_up_that_never_started_leaves_no_cluster_behind`). Tests:
  `each_state_directory_gets_a_volume_of_its_own`,
  `a_kept_cluster_starts_with_its_own_node_count`,
  `a_cluster_from_before_keeps_its_nodes_and_volume`.
- A cluster `local up` makes new (`--fresh`, or nothing kept) forgets the saved
  sessions for its URLs. `login --dev-passkey FILE` never signs up with an existing
  file: the cluster answers `PASSKEY_UNKNOWN_HERE` alike for a removed passkey and
  for a cluster that started over, so a removed passkey stays refused; the error
  says to sign up with a new file. Test:
  `a_cluster_that_started_over_refuses_the_old_passkey_file_and_says_what_to_do`.

```bash
cargo test -p meroctl        # every command group against an in-process 3-node cluster (builds apps/fixture-kv)
cargo test -p meroctl --test app_offline --test cloud --test registry --test dev_new   # no node, no network
cargo run -p meroctl -- --help
```
