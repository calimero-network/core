# team-metrics-macro

Wins, losses and draws per team, on the v2 app SDK (`calimero-app-sdk`,
`wasm32-wasip2`). Methods: `recordWin`, `recordLoss`, `recordDraw` (each
returns the new total), `getWins`, `getLosses`, `getDraws` (`TeamNotFound
{teamId}` before a team's first record). Totals are decimal strings; every
record emits `WinRecorded` / `LossRecorded` / `DrawRecorded {teamId, total}`
to the app's subscribers.

In v1 this app showed `#[derive(Mergeable)]`: replicas merged a struct of
`Counter`s field by field. One Raft log now orders every call, so the stats are
a plain `Table` record and concurrent wins simply add up.

```bash
cargo test --manifest-path apps/team-metrics-macro/Cargo.toml   # harness tests + scenarios/*.json
```
