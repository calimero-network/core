# team-metrics-custom

Wins, losses, draws and a badge bitmask per team, on the v2 app SDK
(`calimero-app-sdk`, `wasm32-wasip2`). The sibling of `team-metrics-macro`,
plus `awardBadge {teamId, badge}` (0-63, else `BadgeOutOfRange {badge}`;
returns the mask), `getBadges` (the mask, a decimal string) and
`getBadgeCount`.

## What changed from v1

In v1 the point of this app was a custom merge rule (`#[app::mergeable]`,
`badges |= other.badges`): without it, two replicas awarding different badges
concurrently resolved last-write-wins and lost one. That rule no longer exists.
One Raft log orders every call and each `awardBadge` reads the current mask
before setting its bit, so two members' awards always end up holding both
badges; there are no replicas to merge.

```bash
cargo test --manifest-path apps/team-metrics-custom/Cargo.toml   # harness tests + scenarios/*.json
```
