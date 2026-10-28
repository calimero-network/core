# calimero-app-host - the host core of `calimero:app`

## Package Identity

- **Crate**: `calimero-app-host` (`publish = false`, prototype); engine-neutral, no wasmtime
- **Entry**: `src/lib.rs`; `core.rs` (`HostCore`), `engine.rs` (`Engine`, `Prepared`), `abi.rs`, `overlay.rs`, `state.rs` (`StateView`, `stored_key`), `time.rs`, `types.rs`
- **Spec**: `docs/specs/app-runtime-v0.14.md` sections 3-5, D16, D19, D39, D44, D46, D47

## What This Crate Does

`HostCore` is the only code that reads or writes app state, applies policy, or serves time and randomness. An `Engine` maps the WIT world one-to-one onto `HostCore` methods (`storage_*`, `range_open/next/close`, `rebuild_*`, context, `emit`, `xcall_send`, `blobs_*`, `action_*`, `wall_ms`, `monotonic_ns`, `random_fill`, `log`) and returns `HostCore::finish(..)` as the `Outcome`.

## Contract

- `HostCore` is `Send + 'static` (a wasmtime `Store`'s data): `HostCore::new(app_id, Arc<dyn StateView>, applied_index, Invocation, Arc<dyn Directory>)` owns or shares its inputs and returns `Err(InvalidInvocation)` for a `Timer` without `TimerCtx` or a `TimerCtx` on any other kind.
- Every refusal is `Err(HostDenied)`; the engine turns it into `Trap(HostDenied)`.
- State writes (`put`, `delete`, `add`), `emit` and `unique_id`: call, timer, init, xcall delivery only. Reads: everything but actions. `start_action`: calls and timers, and returns `Err("actions are not enabled")` in this milestone. xcall, blobs, `action.*` and `rebuild.*` are refused in this milestone.
- At most `MAX_WRITES` (10,000) writes per run; a refused write is not counted.
- Keys are at most `MAX_KEY_BYTES` (1 KiB, before the generation prefix): `storage_get/put/delete/add` and `range_open` bounds over it are `HostDenied`. Values have no separate cap: `MAX_RUN_BYTES` (1 MiB, Ruling 37) bounds what one run writes, counted at each host call (key + value of every `put` and `add`, the key of every `delete`, each overwrite again, and the name, data and audience strings (every account, the team or org id) of every event, Ruling 43); the call that would pass it is `HostDenied("entry too large")` before the bytes are kept (the canonical ABI's lift of one argument, bounded by guest memory, is transient), so the overlay and the events never hold more. The executor still checks the encoded entry against 1 MiB.
- `unique_id()` is `"<id_term>.<id_seq>.<n>"`: `Invocation.id_term` is the leader's Raft term while it runs the call, `id_seq` the leader's per-term execution counter (assigned by the executor, incremented for every execution including re-runs, never repeated within a term), `n` counts from 0 per run. The log index is not used: it is assigned only at proposal, so two runs on the same base could share it.
- `Abi::from_json` requires `abiVersion` in `SUPPORTED_ABI_VERSIONS` (`[1]`; missing is `MissingAbiVersion`, unsupported is `UnsupportedAbiVersion`), ignores unknown keys at the top level and per method, and refuses a known key with a wrongly shaped value (`BadAbi`).
- Keys are stored as `stored_key(GENERATION, key)`; `Write.key` in the diff carries the prefix, keys returned to the guest do not.
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
  - `EMIT_TOO_MANY_ACCOUNTS` ("too many audience accounts"): an `accounts` audience naming more than `MAX_AUDIENCE_ACCOUNTS` (1,000) accounts (Ruling 43).
- The executor, not this crate, refuses a log entry over 1 MiB. That check covers the diff and the events together, so a run within every event limit can still be refused as "entry too large".
- Logged invocations draw randomness from ChaCha20 seeded with `Invocation.seed` and see `entry_ts_ms` on both clocks; views and actions use the node's CSPRNG; actions see the real clock rounded to the second.
- Any status but `Ok` empties the diff and events; the result and logs stay.

## Verify

`cargo test -p calimero-app-host` and `cargo clippy -p calimero-app-host --all-targets -- -D warnings`
