# meroctl

The command line for Calimero nodes (`merod`): one command tree over the
node's API, through `calimero-client` (every request attested and
sealed; the node's quote is judged before anything is sent). Each command
calls the client's generated route methods (`client.orgs().invite_create(..)`,
from the node's OpenAPI document; `crates/client/AGENTS.md`) and prints the
typed answer serialized back to JSON; `raw` and `proxy` send any route through
`Transport`.

| v1 | here | API |
| --- | --- | --- |
| `node add/remove/use/list` | saved nodes, `{home}/meroctl/nodes.json` | (local) |
| `app install/list/get/versions/get-latest-version/pipelines/uninstall/watch` (also `package`) | signed packages (an application); `--path` a `.mpk`, or a dev `.wasm` bundle; `app origins list/add/remove` its registered web origins (its manager's; `add --served` a custom domain the cluster serves the frontend on) | `/packages*`, `/bundles`, `/package-origins/{package}` |
| `context create/list/get/delete/update/watch/alias/use` | app instances; `use` is the alias `default`; also `access`, `transfer`, `metadata`, `storage`, `abi`, `presence` | `/apps*`, `/auth/aliases*` |
| `call` | `call --context C METHOD --args JSON [--view]` | `/apps/{app}/intents`, `/views` |
| `blob list/upload/download/info/delete` | a context's blobs | `/apps/{app}/blobs*` |
| `file upload FILE -c CONTEXT` | a context's sealed files: one request up to 16 MiB, else resumable parts (`Client::upload_file`) | `/apps/{app}/files*` |
| `file download CONTEXT ID -o OUT` | a sealed file's plaintext, streamed to `OUT.part` and renamed onto `OUT` only after every check passed (`Client::export_file_to`); `pipeline download` does the same per output | `/apps/{app}/files/{id}/export`, `.../segments/{k}` |
| `run logs APP RUN [--attempt N]`, `pipeline logs APP RUN --step S [--index I]` | print a compute run's module log (stdout, stderr and `log` lines) read through the files export; the latest attempt unless `--attempt`; a pipeline step's run is its `--index`th | `/apps/{app}/runs/{run}/logs`, `/apps/{app}/files/{id}/export` |
| (new) `pipeline validate/estimate/run/retry/get/cancel/watch/runs/receipt/logs/download` | a context's pipeline runs: check a pipeline source, estimate, start, follow (`watch`, SSE), the signed receipt (`receipt --verify`) | `/pipelines/validate`, `/apps/{app}/pipelines/*`, `/apps/{app}/pipeline-runs*` |
| `namespace ...` | organizations: members, invites, groups, metadata, capabilities, outbound and release approvals | `/orgs*`, `/invites/*` |
| `group ...` | teams: members, metadata, reparent, subgroups, settings; `group upgrade trigger/status/retry` (org-wide runs) | `/orgs/{org}/teams*`, `/orgs/{org}/upgrade*` |
| `account ...` | the account: devices (passkeys), API keys, usage, approvals | `/auth/*` |
| `dev start/watch` | install, create, keep pushing a build | |
| `network status` | `/health` | |
| (new) `login`, `logout [--all]`, `whoami`, `attest [--context]` | file passkeys on dev clusters; cluster verification | `/auth/*`, `/attestation` |
| (new) `attest-release [--file F --nonce N]` | verify a `merod attest` release attestation (no session, no sealed channel) | `/attestation/release` |
| (new) `proxy [--listen ADDR] [--allow-remote] [--openapi FILE]` | the API as plain HTTP on loopback, each request through the attested, sealed `Transport` (`proxy.rs`) | every route; `GET /openapi.json` |
| (new) `ops ...` | the operator, `--ops-token`: status, cluster, metrics, placement, add-node, remove-node, move, backup, restore, backup-key, recovery-keygen, recover, pools/set-pools, prices/set-prices, quotas/set-quotas, release, outbound, packages/install/uninstall, upgrade, upgrade-namespace | `/ops/*` |

Dropped, with no meaning here: `context sync`, `group sync` (Raft replicates),
`context identity`, `account pair-*`/`seal-to`/`device-init` (a passkey is the
device), `peers` (no gossip), `tee fleet-join` (`ops add-node`).

- Sessions are kept per node URL in `{home}/meroctl/sessions.json` (0600) and
  refreshed when they expire. A context argument is an alias of the account or
  an id (`Env::app`).
- What an owner approves (`context update`, `context transfer`, `namespace delete`, approvals,
  `group upgrade trigger/retry`, `app origins add/remove`) takes `--dev-passkey FILE` on a development
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

```bash
cargo test -p meroctl        # every command group against an in-process 3-node cluster (builds apps/v2-fixtures/kv)
cargo run -p meroctl -- --help
```
