# team-metrics

Wins, losses, draws and a badge bitmask per team, on the v2 app SDK
(`calimero-app-sdk`, `wasm32-wasip2`): `recordWin`, `recordLoss` and
`recordDraw {teamId}` upsert the team and return its new total (emitting
`WinRecorded`, `LossRecorded` or `DrawRecorded`); `getWins`, `getLosses` and
`getDraws` read them (`TeamNotFound {teamId}` for an unknown team);
`awardBadge {teamId, badge}` (0-63, else `BadgeOutOfRange {badge}`) returns the
mask, `getBadges` reads it (a decimal string) and `getBadgeCount` counts it.

## What changed from v1

v1 had two apps here. One derived `Mergeable` for a struct of `Counter`s,
which storage merged field by field across replicas; the other added the
badges with a hand-written merge rule
(`#[app::mergeable]`, `badges |= other.badges`), without which two replicas
awarding different badges concurrently resolved last-write-wins and lost one.
Neither exists any more. One Raft log orders every call: two members recording
a win at once are two entries that each add one, and each `awardBadge` reads
the current mask before setting its bit, so two members' awards always end up
holding both badges. There are no replicas to merge, so the two apps are one.

```bash
cargo test --manifest-path apps/team-metrics/Cargo.toml   # harness tests + scenarios/*.json
```

`tests/scenarios.rs` runs `scenarios/team-metrics.json` (the former merobox
workflow, with the badges) in the harness; the cluster runs the same file.
