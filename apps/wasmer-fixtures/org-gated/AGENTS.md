# org-gated (wasmer fixture)

A wasmer-runtime fixture, not an example app: notes an organization keeps,
gated by `env::caller_org()` — members read the pinned note, admins write it,
a team's members write that team's note. `whoami` returns what the node told
the app about the caller.

It stays on the v1 SDK (`calimero-sdk`, `wasm32-unknown-unknown`) because
`crates/node/tests/cluster.rs` (`apps_see_the_callers_organization_role_and_teams`)
loads it as `org_gated.wasm` to test the wasmer executor's `caller_org`. The
example app on the component SDK is `apps/org-gated`; the component fixture the
node's component tests use is `apps/v2-fixtures/org-gated`.

```bash
cargo build -p org-gated --target wasm32-unknown-unknown --profile app-release
```
