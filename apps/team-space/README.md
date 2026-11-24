# team-space

A team's shared space on the component SDK: a pinned note and team notes, a status
line, settings, an owned config and roles. It shows the layers of access control in
one app, each an explicit check against the caller (the rules are stated in
`src/lib.rs`'s doc comment):

1. **The node** decides who reaches the app at all (its access).
2. **Organization standing** (`env::org_role`, `env::in_team`): members read the pinned
   note, admins write it; a team's members write that team's note; members announce.
3. **The app's own rows**: one account owns the config and hands it on with
   `transfer`; writer sets (rows of the `writers` table, one per set and account) guard
   the status line (`value`), the settings (`map`) and the role registry (`roles`), and
   a writer of a set replaces that set with `rotateWriters`.

The app owner, who runs `init`, owns the config and is the one writer of every set.

It is also the node's org fixture: `crates/node/tests/it/component_cluster.rs` and
`fixtures.rs` build it, and `shared_storage.rs` runs its writer sets on a cluster.

## ABI

| Method | Kind | Params | Returns | Rule |
| --- | --- | --- | --- | --- |
| `whoami` | view | `candidates?: [string]` | `{ org, role, teams } \| null` | the caller's standing; `teams` are the candidates it is in |
| `role` | view | | `string \| null` | the caller's org role |
| `myAccount` | view | | `string` | the caller's account, as an owner, writer set or grant names it |
| `pinned` | view | | `string` | members and above (`MembersOnly`) |
| `setPinned` | call | `text` | | admins and the owner (`AdminsOnly`) |
| `announce` | call | `text` | | members and above; `Announced` to the org |
| `setTeamNote` | call | `team, text` | | that team's members (`TeamOnly`) |
| `teamNote` | view | `team` | `string \| null` | |
| `setConfig` | call | `value` | | the config's owner (`NotOwner`) |
| `getConfig` | view | | `string` | `""` until set |
| `owner` | view | | `string \| null` | the config's owner |
| `transfer` | call | `newOwner` | | the config's owner (`NotOwner`) |
| `setShared` | call | `value` | | writers of `value`; `SharedSet` |
| `getShared` | view | | `string` | the status line, `""` until set |
| `mapSet` | call | `key, value` | | writers of `map` |
| `mapGet` | view | `key` | `string \| null` | a setting |
| `grantRole` | call | `role, who` | | writers of `roles` |
| `tryGrantRole` | call | `role, who` | `bool` | `false` instead of `NotWriter` |
| `revokeRole` | call | `role, who` | | writers of `roles` |
| `hasRole` | view | `role, who` | `bool` | |
| `rotateWriters` | call | `newWriters: [string], field?` | | writers of that set (`value` by default); `WritersRotated { field, count }` |
| `tryRotateWriters` | call | `newWriters, field?` | `bool` | `false` instead of `NotWriter` |
| `writerCount` | view | `field?` | `u64` | accounts in that set (`value` by default) |

Errors: `MembersOnly`, `AdminsOnly`, `TeamOnly`, `NotOwner`, `NotWriter { field }`,
`NoWriters`, `TooManyWriters { max }`.

## Tests

`cargo test` runs `tests/harness.rs` (each rule and its refusals) and every
`scenarios/*.json` in the org of the `.org.json` beside it; the node's cluster
scenarios run the same files on three nodes:

- `scenarios/team-space.json`: organization standing, the owned config and roles, and
  a member removed from the org.
- `scenarios/writer-sets.json`: the former merobox workflow `test_shared_storage.yml`,
  then v1's forged writes (a non-writer setting the value or a setting, granting a
  role, or rotating a set), each refused by the writer check.
- `scenarios/cluster/team-space.json`: a team created, joined and left mid-run.
