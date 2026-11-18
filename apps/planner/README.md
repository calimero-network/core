# planner

A team planner that brings its own calendar. Its `Cargo.toml` declares

```toml
[package.metadata.calimero.uses]
calendar = { package = "com.calimero.example.calendar", range = "^0.1", mode = "create" }
```

so creating a planner from its release first creates a calendar in the same organization,
owned by the same person (`<instance>.calendar`), and links it. The planner reads it with
`env::linked("calendar")` and schedules on it as itself, so the agenda is the planner's own.
`upgrade-unit` (or the consent page's `upgradeUnit`) upgrades the two together.

The calendar is called through `uses::calendar`, a typed client `build.rs` generates from
the calendar's ABI vendored in `calimero/abi/calendar.json` and pinned in `calimero.lock`
(`meroctl app uses update calendar --from ../calendar` refreshes them).

```bash
cargo test --manifest-path apps/planner/Cargo.toml
```

Guide: [Build on other apps](../../docs/src/content/docs/build/guides/other-apps.mdx).
