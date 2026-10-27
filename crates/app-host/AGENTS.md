# calimero-app-host - the host core of `calimero:app`

## Package Identity

- **Crate**: `calimero-app-host` (`publish = false`, prototype); engine-neutral, no wasmtime
- **Entry**: `src/lib.rs`; `core.rs` (`HostCore`), `engine.rs` (`Engine`, `Prepared`), `abi.rs`, `overlay.rs`, `state.rs` (`StateView`, `app_key`), `time.rs`, `types.rs`
- **Spec**: `docs/specs/app-runtime-v0.13.md` sections 3-5, D16, D19, D39, D44, D46, D47

## What This Crate Does

`HostCore` is the only code that reads or writes app state, applies policy, or serves time and randomness. An `Engine` maps the WIT world one-to-one onto `HostCore` methods (`storage_*`, `range_open/next/close`, `rebuild_*`, context, `emit`, `xcall_send`, `blobs_*`, `action_*`, `wall_ms`, `monotonic_ns`, `random_fill`, `log`) and returns `HostCore::finish(..)` as the `Outcome`.

## Contract

- `HostCore` is `Send + 'static` (a wasmtime `Store`'s data): `HostCore::new(app_id, Arc<dyn StateView>, applied_index, Invocation, Arc<dyn Directory>)` owns or shares its inputs and returns `Err(InvalidInvocation)` for a `Timer` without `TimerCtx` or a `TimerCtx` on any other kind.
- Every refusal is `Err(HostDenied)`; the engine turns it into `Trap(HostDenied)`.
- State writes (`put`, `delete`, `add`), `emit` and `unique_id`: call, timer, init, xcall delivery only. Reads: everything but actions. `start_action`: calls and timers, and returns `Err("actions are not enabled")` in this milestone. xcall, blobs, `action.*` and `rebuild.*` are refused in this milestone.
- At most `MAX_WRITES` (10,000) writes per run; a refused write is not counted.
- Keys are stored as `app_key(GENERATION, key)`; `Write.key` in the diff carries the prefix, keys returned to the guest do not.
- `storage_add` reads the current value host-side; it must be a shortest-form CBOR integer (absent = 0); the sum is stored in the same encoding.
- A cursor reads the state as of `range_open`: the committed state plus the run's writes made before it opened. The overlay stamps every write with a run sequence number and keeps each key's versions (bounded by `MAX_WRITES`); a cursor uses, per key, the newest version at or before its opening sequence, else the `StateView`. `storage_get` and the diff use the newest version. A scan that moves records ahead of itself therefore never meets them again.
- Ranges are lazy and hold no iterator between batches: every `range_next` re-ranges the `StateView` (forward from `last ‖ 0x00`, reverse up to `last`) and the overlay from the last returned key, reading one committed entry ahead (at most `max + 1` entries held); a batch is capped at `MAX_RANGE_BATCH`.
- `emit` returns the WIT `result<_, string>` inside `Ok`; a policy refusal is the outer `HostDenied`. Soft errors, as fixed `pub const` strings, leave no event recorded:
  - `EMIT_TOO_MANY` ("too many events"): more than `MAX_EVENTS` (1,000) per run.
  - `EMIT_NAME_TOO_LONG` ("event name too long"): name over `MAX_EVENT_NAME_BYTES` (100).
  - `EMIT_DATA_TOO_LARGE` ("event data too large"): data over `MAX_EVENT_DATA_BYTES` (16 KiB).
  - `EMIT_BAD_AUDIENCE` ("malformed audience"): an empty `accounts` list or account id, or an empty team or org id.
  - `EMIT_UNKNOWN_TEAM` ("unknown team"): `Directory::team_exists` is false.
  - `EMIT_FOREIGN_ORG` ("audience names another org"): `org-members`/`org-admins` naming an org other than the app's own.
- The executor, not this crate, refuses a log entry over 1 MiB. That check covers the diff and the events together, so a run within every event limit can still be refused as "entry too large".
- Logged invocations draw randomness from ChaCha20 seeded with `Invocation.seed` and see `entry_ts_ms` on both clocks; views and actions use the node's CSPRNG; actions see the real clock rounded to the second.
- Any status but `Ok` empties the diff and events; the result and logs stay.

## Verify

`cargo test -p calimero-app-host` and `cargo clippy -p calimero-app-host --all-targets -- -D warnings`
