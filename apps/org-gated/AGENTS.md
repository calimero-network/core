# org-gated

Release-v2 example: notes an organization keeps, gated by `env::caller_org()` —
members read the pinned note, admins write it, a team's members write that
team's note. `whoami` returns what the node told the app about the caller.

Built for the cloud node's tests (`crates/cloud-node/tests/cluster.rs`,
`apps_see_the_callers_organization_role_and_teams`):

```bash
cargo build -p org-gated --target wasm32-unknown-unknown --profile app-release
```
