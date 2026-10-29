# meroctl

The command line for Calimero nodes (`merod`): v1's command tree over the
node's API, through `calimero-client` (every request attested and
sealed; the node's quote is judged before anything is sent).

| v1 | here | API |
| --- | --- | --- |
| `node add/remove/use/list` | saved nodes, `{home}/meroctl/nodes.json` | (local) |
| `app install/list/get/versions/get-latest-version/uninstall/watch` | signed packages (an application); `--path` a `.mpk`, or a dev `.wasm` bundle | `/packages*`, `/bundles` |
| `context create/list/get/delete/update/watch/alias/use` | app instances; `use` is the alias `default`; also `access`, `transfer`, `metadata`, `storage`, `abi`, `presence` | `/apps*`, `/auth/aliases*` |
| `call` | `call --context C METHOD --args JSON [--view]` | `/apps/{app}/intents`, `/views` |
| `blob list/upload/download/info/delete` | a context's blobs | `/apps/{app}/blobs*` |
| `namespace ...` | organizations: members, invites, groups, metadata, capabilities, outbound and release approvals | `/orgs*`, `/invites/*` |
| `group ...` | teams: members, metadata, reparent, subgroups, settings; `group upgrade trigger/status/retry` (org-wide runs) | `/orgs/{org}/teams*`, `/orgs/{org}/upgrade*` |
| `account ...` | the account: devices (passkeys), API keys, usage, approvals | `/auth/*` |
| `dev start/watch` | install, create, keep pushing a build | |
| `network status` | `/health` | |
| (new) `login`, `logout [--all]`, `whoami`, `attest [--context]` | file passkeys on dev clusters; cluster verification | `/auth/*`, `/attestation` |
| (new) `attest-release [--file F --nonce N]` | verify a `merod attest` release attestation (no session, no sealed channel) | `/attestation/release` |
| (new) `ops ...` | the operator, `--ops-token`: status, metrics, placement, add-node, remove-node, move, backup, quotas, release, outbound, packages/install/uninstall, upgrade, upgrade-namespace | `/ops/*` |

Dropped, with no meaning here: `context sync`, `group sync` (Raft replicates),
`context identity`, `account pair-*`/`seal-to`/`device-init` (a passkey is the
device), `peers` (no gossip), `tee fleet-join` (`ops add-node`).

- Sessions are kept per node URL in `{home}/meroctl/sessions.json` (0600) and
  refreshed when they expire. A context argument is an alias of the account or
  an id (`Env::app`).
- What an owner approves (`context update`, `context transfer`, `namespace delete`, approvals,
  `group upgrade trigger/retry`) takes `--dev-passkey FILE` on a development
  cluster (`Env::post_approved`, `delete_approved`); without it the request is
  sent as is and the node answers `412 CONFIRM_REQUIRED`: on a real cluster
  the owner approves in the console.
- Output is JSON; commands that stream (watch, presence stream, metrics,
  downloads) write their own and answer `null`, which is not printed.
- Exit status: `0` done, `1` any error (`main` returns it: `Error: ...`), `2` usage
  (clap), `77` (`context::ACCESS_LOST_EXIT`, sysexits' `EX_NOPERM`) when `context
  watch` or `context presence stream` ends on the node's `access_lost` frame
  (`calimero_client::access_lost`; `main` maps `context::AccessLost` to it and
  prints `access lost: {status} {code} ({message})`). Regression:
  `tests/access_lost.rs` (its own binary: two local clusters at once in one
  binary can take each other's ports).

```bash
cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release   # test fixture
cargo test -p meroctl        # every command group against an in-process 3-node cluster
cargo run -p meroctl -- --help
```
