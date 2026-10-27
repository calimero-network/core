# Rooms

The first reference app on the v2 app SDK (spec `docs/specs/app-runtime-v0.14.md`, section 12). Members of an org book meeting rooms in 15-minute slots.

- No double bookings: each held slot is one `Slot` record keyed (room, start); a clash fails the whole call with `Taken`.
- A timer (`releaseNoShows`, every minute) releases bookings still unchecked 15 minutes after their start (at most 500 per tick).
- `adminRelease` needs a fresh passkey confirmation (`#[app::confirm]`).
- Titles are visible only to the holder and org admins.
- Booking ids are `b{env::unique_id()}`, not a counter.

Constants: `SLOT_MS = 900_000`, `CHECK_IN_GRACE_MS = 900_000`, at most 8 slots per booking. `u64` values travel as decimal strings in JSON.

## ABI

Every method needs an org member (`LoginRequired` for anonymous, `Forbidden` for guests); admin-only where noted.

| Method | Kind | Params | Returns | Notes |
| --- | --- | --- | --- | --- |
| `init` | init | | | |
| `addRoom` | call | `id: string, name: string, capacity: u32` | | admin; `Duplicate { index: "id" }` |
| `book` | call | `roomId: string, start: u64, slotCount: u32, title: string` | booking id | `Invalid { reason }` (start not a slot multiple, slotCount outside 1..8), `NotFound` (room), `Taken { start }` (first held slot) |
| `checkIn` | call | `id: string` | | holder only, else `NotFound` |
| `cancel` | call | `id: string` | | holder only, else `NotFound`; frees the slots |
| `adminRelease` | call, confirm | `id: string, reason: string` | | admin; frees the slots |
| `releaseNoShows` | timer `1m`, allow system | | | releases unchecked bookings with `start + CHECK_IN_GRACE_MS <= now`, reason `"no-show"` |
| `availability` | view | `roomId: string, dayStart: u64` | `[SlotView]` | held slots in the 24 h from `dayStart`; `title` is null unless holder or admin |
| `myBookings` | view | | `[Booking]` | earliest first |
| `booking` | view | `id: string` | `Booking` | `NotFound` unless holder or admin |

`Booking { id, roomId, holder, start, end, title, checkedIn }`; `SlotView { start, end, bookingId, title, mine }`.

Errors: `LoginRequired`, `Forbidden`, `NotFound`, `Taken { start }`, `Invalid { reason }`, `Duplicate { index }`.

Events: `SlotsTaken { roomId, start, end }` to the org on every booking; `Booked { id, title }` to the booker; `BookingReleased { id, reason }` to the holder on a timer or admin release.

## Tests

`cargo test` in this directory builds the component for `wasm32-wasip2` and runs `scenarios/rooms.json` (the shared scenario, also run on the 3-node cluster and by the TypeScript build) on the real engine. The crate is standalone (its own `[workspace]`), like the SDK's sample guest.

On the 3-node cluster: `cargo test --release -p calimero-cloud-node --test rooms` runs the same scenario (step `i` on node `i % 3`), the contention test and the follower reads; `crates/cloud-node/tests/fault/rooms_fault.py` is the fault test. `cargo test -- --nocapture` prints the fuel of one `book`.
