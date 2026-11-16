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

| v1 | here | API |
| --- | --- | --- |
| `node add/remove/use/list` | saved nodes, `{home}/meroctl/nodes.json` | (local) |
| `app install/list/get/versions/get-latest-version/pipelines/uninstall/watch` (also `package`) | signed packages (an application); `--path` a `.mpk`, or a dev `.wasm` bundle; `app origins list/add/remove` its registered web origins (its manager's; `add --served` a custom domain the cluster serves the frontend on) | `/packages*`, `/bundles`, `/package-origins/{package}` |
| `instance create/list/get/delete/update/watch/alias/use` | app instances; `use` is the alias `default`; also `access`, `transfer`, `metadata`, `storage`, `abi`, `presence` | `/apps*`, `/auth/aliases*` |
| `call` | `call --instance I METHOD --args JSON [--view]` | `/apps/{app}/intents`, `/views` |
| `blob list/upload/download/info/delete` | an instance's blobs | `/apps/{app}/blobs*` |
| `file upload FILE -i INSTANCE` | an instance's sealed files: one request up to 16 MiB, else resumable parts (`Client::upload_file`) | `/apps/{app}/files*` |
| `file download INSTANCE ID -o OUT` | a sealed file's plaintext, streamed to `OUT.part` and renamed onto `OUT` only after every check passed (`Client::export_file_to`); `pipeline download` does the same per output | `/apps/{app}/files/{id}/export`, `.../segments/{k}` |
| `run logs APP RUN [--attempt N]`, `pipeline logs APP RUN --step S [--index I]` | print a compute run's module log (stdout, stderr and `log` lines) read through the files export; the latest attempt unless `--attempt`; a pipeline step's run is its `--index`th | `/apps/{app}/runs/{run}/logs`, `/apps/{app}/files/{id}/export` |
| (new) `pipeline validate/estimate/run/retry/get/cancel/watch/runs/receipt/logs/download` | an instance's pipeline runs: check a pipeline source, estimate, start, follow (`watch`, SSE), the signed receipt (`receipt --verify`) | `/pipelines/validate`, `/apps/{app}/pipelines/*`, `/apps/{app}/pipeline-runs*` |
| `org ...` | organizations: members, invites, teams, metadata, capabilities, outbound and release approvals | `/orgs*`, `/invites/*` |
| `team ...` | teams: members, metadata, reparent, subteams, settings; `team upgrade trigger/status/retry` (org-wide runs) | `/orgs/{org}/teams*`, `/orgs/{org}/upgrade*` |
| `account ...` | the account: devices (passkeys), API keys, usage, approvals | `/auth/*` |
| `dev start/watch` | install, create, keep pushing a build (`dev new` is a hidden alias of `app new`) | |
| (new) `app new/build/codegen/abi show\|diff/signer new\|id/pack/sign/inspect/publish` | no node: start from `calimero-network/apps` (`dev.rs::new`, a sparse clone, plus an `AGENTS.md` for coding agents (`agents_md`) and a `CLAUDE.md` importing it; test `dev_new_starts_a_project_from_an_app_of_the_apps_repo`), `calimero-app-build`'s builder and `abi_tools`, `mero-sign`'s library, `calimero-bundle` (the frontend checked with `validate` and `check_frontend_html`), `pack --service NAME=PATH` (or the crate's `services`) writes a multi-service manifest, and `pack` opens what it wrote with `VerifiedBundle` before writing it; the registry's `POST /api/bundles/push` (`--org`: the Cloud's `POST /api/registry/token` first) | registry, Cloud |
| (new) `local up/down/status/dev/test/compute run` | `merod local` (the release's image by default, `docker_ready` checking Docker first; `--local` or `MEROD`: the binary; `--registry-url`, default the public registry) in the background, state in `--dir`/`meroctl-local.json`, saved as node `local`; `dev` builds, uploads a raw bundle as a fresh deployer, writes `app/.env.development.local`, runs `pnpm dev` on :5173 and rebuilds on save; `compute run` is `mero-compute` (feature `compute`, default on) | `/bundles` |
| (new) `cloud login/logout/orgs/clusters list\|create/use`, `cloud compute pipeline\|logs` | the Cloud manager (mdma) with a console session in `{home}/meroctl/cloud.json` (refreshed from `X-MDMA-Session-Refresh`); `use` saves a cluster's node as the default; `cloud compute` is the old top-level `pipeline` and `run logs` (hidden aliases) | `/api/orgs`, `/api/clusters` |
| `network status` | `/health` | |
| (new) `login`, `logout [--all]`, `whoami`, `attest [node\|app INSTANCE]` | file passkeys on dev clusters; cluster verification | `/auth/*`, `/attestation` |
| (new) `attest release [--file F --nonce N]` (`attest-release` hidden) | verify a `merod attest` release attestation (no session, no sealed channel) | `/attestation/release` |
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
  `team upgrade trigger/retry`, `app origins add/remove`) takes `--dev-passkey FILE` on a development
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
  printed.

- `main` runs the commands that need no node (`node`, `local`, `cloud` but
  `cloud compute`, `app`'s offline ones: `AppCmd::is_offline`) before it
  resolves a node, so they never read `--node`/`--api`. The old spellings
  (`dev new`, `pipeline`, `run logs`, `attest-release`, `attest --context`, `context` (now `instance`) and `--context`/`-c` (now `--instance`/`-i`),
  `namespace`/`ns` (now `org`, with `teams`, `create-team`), `group` (now `team`, with `subteams`),
  `--namespace`/`--group` (now `--org`/`--team`) and `ops upgrade-namespace` (now `upgrade-org`))
  stay as hidden aliases for one release, and each one an invocation uses prints a warning on
  stderr naming its replacement (`old_spellings`, test
  `old_spellings_are_named_with_their_replacements`); `old_spellings_still_parse` and
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
  `merod.log`) keeps 3 nodes and the port's volume (`made`, `plan`). Tests:
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
cargo test -p meroctl        # every command group against an in-process 3-node cluster (builds apps/v2-fixtures/kv)
cargo test -p meroctl --test app_offline --test cloud --test dev_new   # no node, no network
cargo run -p meroctl -- --help
```
