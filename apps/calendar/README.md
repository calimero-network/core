# calendar

Each caller's own events: a person's (an API key of theirs counts as them) and an app's
calling as itself. It is the app the units examples build on, and it knows nothing about
them. The one thing it does for a call made through another app (`env::via()`) is keep the
person's private events out of it.

- `planner` creates a calendar with each of its instances (`mode = "create"`).
- `booking` is attached to a calendar its owner already runs (`mode = "attach"`) and, with
  a person's grant, adds events to their own calendar as them.

```bash
cargo test --manifest-path apps/calendar/Cargo.toml
```

Guide: [Build on other apps](../../docs/src/content/docs/build/guides/other-apps.mdx).
