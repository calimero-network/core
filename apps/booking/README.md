# booking

A booking page on a calendar its owner already runs. Its `Cargo.toml` declares

```toml
[package.metadata.calimero.uses]
calendar = { package = "com.calimero.example.calendar", range = "^0.1", mode = "attach", methods = ["addEvent", "myEvents"] }
```

1. The owner attaches their calendar: `PUT /apps/{booking}/links/calendar { "app": "<calendar>" }`.
2. A person grants booking those two methods: its frontend asks the consent page with
   `delegate { app, target }`.
3. Their `book` then adds the event to **their own** calendar as them, and `freeSlots`
   reads their own events; the calendar keeps their private ones out (`env::via()`).

Without a grant the same calls run as booking itself.

The calendar is called through `uses::calendar`, a typed client `build.rs` generates from
the calendar's ABI vendored in `calimero/abi/calendar.json` and pinned in `calimero.lock`.
A private booking (`book { at, title, private: true }`) is refused by the calendar with its
own `PrivateThroughApp`, which booking matches as `uses::calendar::Error::PrivateThroughApp`
and answers as `PrivateNotAllowed`. After the calendar's ABI changes:

```bash
meroctl app uses update calendar --from ../calendar
```

```bash
cargo test --manifest-path apps/booking/Cargo.toml
```

Guide: [Build on other apps](../../docs/src/content/docs/build/guides/other-apps.mdx).
