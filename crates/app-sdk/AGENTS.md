# calimero-app-sdk - the Rust app SDK runtime

## Package Identity

- **Crate**: `calimero-app-sdk` (`publish = false`, prototype); builds for the host (tests) and `wasm32-wasip2` (apps)
- **Entry**: `src/lib.rs` (+ `prelude`); `app.rs` (the `app` attributes from `calimero-app-sdk-macros`, `AppState`, `AppEvent`, `bail!`), `private.rs` (`__private`: the generated `handle`, argument decoding, the ABI assembly; not an API), `encoding/{key,value}.rs`, `store.rs` (+ `store/{mem,host}.rs`), `table.rs`, `collections/{map,counter}.rs`, `env.rs` (+ `env/{wasm,mock}.rs`), `guards.rs`, `error.rs`, `json.rs`, `bindings.rs` (wasm32 only), `testing/{mod,directory,scenario}.rs` (feature `testing`, host only)
- **Spec**: `docs/specs/app-runtime-v0.14.md` section 7, the SDK reference, D9, D10, D20, D47, D48, D50, D52

## What This Crate Does

The shared storage encoding (order-preserving tuple keys, deterministic CBOR values), the `Store` trait over the host's ordered key-value storage, tables with indexes, unique constraints and counts, `UnorderedMap`, `Counter`, `env` and the member/admin guards. It re-exports the `app` attributes and `#[derive(Record)]` (see `crates/app-sdk-macros/AGENTS.md`); apps write `use calimero_app_sdk::prelude::*;`. The `testing` feature adds the test harness (host target only).

## The generated `handle` (`__private::handle`)

- Method lookup by ABI (camelCase) name; unknown → `AppError UnknownMethod`.
- Kind: a view may run as a call (or xcall delivery); a call, timer or init method run as a view → `NotAView`; any other mismatch, and `action` / `migrate` (not in this version) → `WrongKind`.
- Callers (D13), checked after the kind, except for `init` (runs as the owner): `System` needs `allow(system)` and `App(id)` needs `allow(app = id)`, else `Forbidden`; `Public` needs `public`, else `LoginRequired`.
- Args: a JSON object keyed by camelCase parameter names, empty args = `{}`, unknown keys refused; `u64`/`i64` (and `Option`) params are canonical decimal strings (no `+`, no leading zeros, no `-0`) and `Vec<u8>` (and `Option`) params base64url without padding (D20); anything malformed → `BadArgs` (`{"message": ..}`).
- Timers: `#[app::every]` implies `allow(system)` (Ruling 26), so the ABI says `allowSystem: true` on every timer.
- Timer ticks: for `kind = timer` the engine passes the tick as `args`, `{"scheduled":"<ms>","previous":"<ms>"|null}`; `handle` sets `env::timer()` from it (and clears it for every other kind).
- Results: JSON; a `u64`/`i64` (or `Option`) result is a decimal string, a `Vec<u8>` (or `Option`) result base64url; `()` and `init` give `null`. Records inside results: their `u64`/`i64` fields are decimal strings and `Vec<u8>` fields base64url too (`json::*_auto`).

## D20 conversion rule (Ruling 27)

The macros convert exactly these types, at the top level of a parameter, a result, a record field, or an event or error field: `u64`, `i64`, `Vec<u8>` and `Option` of each. The same types nested inside another type the macro can see (`Vec<u64>`, `Option<Vec<i64>>`, `Option<Option<u64>>`, tuples, arrays, maps, `Vec<Vec<u8>>`) are a compile error naming D20 ("u64 inside Vec<u64> would lose precision in JSON (D20); wrap it in a Record or use a string"). Limit: the macro sees only the tokens it is given. A user struct with plain serde, or a `type` alias, is opaque to it, so a `u64` inside one still reaches JSON as a number; use a `Record` (its derive converts the fields) or a string.
- Errors: the wire `app-error` is `{ code, message }` with `message` = the fields as JSON (`{}` when none), e.g. `Taken` / `{"start":"7"}`.

## Rules

- Key layouts are the cross-language contract (the TypeScript SDK must produce the same bytes): change them only with the spec.
- Table counts and `Counter::add` use `Store::add`, never read-modify-write (D47).
- Keys over 1 KiB are refused with `StorageError::KeyTooLong` before any store call (D50).
- Queries read `limit + 1` entries through the cursor and no more.
- `bindings.rs` declares the `handle` export but never exports it: the code `#[app::logic]` generates does, through `__private::bindings::__calimero_export_handle!` and `__private::Export<State>`.
- `#[app::error]` and `#[app::event]` must live in the `#[app::logic]` module (their ABI constants are picked up there; elsewhere the ABI lists none), and one module takes one of each: a second `#[app::error]` enum in a module is refused (`__CALIMERO_ABI_ERRORS` defined twice, pinned by `tests/ui/second_error.rs`).
- `env::emit(event, audience)` takes an `#[app::event]` value (spec); `env::emit_raw(name, data, audience)` takes any serde value.
- `env::mock` (host target) is the stand-in for the host in tests.

## Encoding contract (the TypeScript SDK must match)

- **Keys**: tag byte per part (none 0x00, bool 0x10, int 0x20, string 0x30, bytes 0x40); ints are sign-flipped big-endian 16 bytes; strings and bytes escape `0x00` as `0x00 0xFF` and end with `0x00 0x00`.
- **Key length (D50, ruling 24)**: the host refuses keys over 1,024 bytes. The SDK caps every key it stores (record, index, unique, count, map, counter) at 1,023 (`MAX_SDK_KEY_BYTES`), so derived bounds (`last ++ 0x00`, `prefix ++ 0xFF`) always fit. Every range bound and decoded cursor is checked before any store call: bounds give `KeyTooLong`, a cursor gives `BadCursor`.
- **Unique constraints use SQL NULL semantics (ruling 23)**: when any field of a unique constraint is `None`, no unique entry is written or checked (insert, upsert, update, remove). Two records with no email both insert; the same `Some(email)` still clashes.
- **Values**: deterministic CBOR (RFC 8949 core). Field names are camelCase (applied by the `#[app::state]`/record macro, not by the SDK). `None` is written as CBOR null, never omitted. An integral float stays a float (`1.0` is `f93c00`), floats use the shortest exact form, `-0.0` is preserved, NaN is refused, map keys sort by encoded bytes, lengths are definite.
- **JSON (D20, ruling 25)**: `u64`/`i64` in method args and results, event data and `AppError` fields are canonical decimal strings; bytes are base64url without padding; use `calimero_app_sdk::json::{u64_str, i64_str, opt_u64_str, opt_i64_str, bytes_b64url, opt_bytes_b64url}` (the macros apply them). Record fields use `json::{u64_auto, i64_auto, bytes_auto, ..}`: an integer or a byte string (major type 2) in CBOR (not human-readable), a decimal string or base64url in JSON.
- **Cursors** are the base64url of the last storage key, bound to the index and prefix, not signed.
- **Prefixes** longer than the index or count fields are refused (`PrefixTooLong`); `limit: 0` is an empty page.

## Test harness (`testing` feature, spec section 10)

- `TestApp::new(component, Org, now_ms)` prepares the component on the real `WasmtimeEngine` (one engine and compile cache per test process) and runs `init` as `"owner"`. `t.as_(a)` / `anonymous()` / `system()` / `api_key(id, account)` give an `As` with `call`, `view`, `confirmed()`; `Failure` is `App { code, fields }`, `Trap(TrapKind)` or `ConfirmRequired`.
- It plays the executor: the diff is applied only on `Ok`; each logged invocation gets a fresh `id_seq` (`id_term` 1) and a seed derived from `t.seed(n)`; views see the last logged entry's timestamp (D19). Before running it applies the ABI checks a node makes: a `confirm` method needs `.confirmed()` (else `ConfirmRequired`) and is never a view (`NotAView`); a non-`public` method refuses anonymous (`LoginRequired`), a method without `allowSystem` refuses `system()` (`Forbidden`).
- `t.tick(method)` runs the timer as `System` for the slot at the current time; `previous` is the last tick of the same method. A tick that does not time out counts as applied (D34).
- Events: `events_for(account)` resolves audiences when asked (R9); `remove_member` first fixes who org events so far reached, so earlier events stay with the leaver and later ones do not reach it. 
- `export_state()` is deterministic CBOR (RFC 8949 4.2.1): a map of stored key (generation prefix included) to value, byte strings, ordered by encoded key bytes. The TypeScript harness must produce the same bytes.
- What the harness org supports: one org (`ORG_ID`) with the owner (`"owner"`, role Owner), `admins`, `members`, `guests` (role checks, `org_role`, `orgs_of`), and `Org::teams([("id", ["acct", ..])])` (`in_team`, `team_exists`, `Team` audiences; a team needs no org role). `Org` audiences reach members, admins and the owner, not guests; `OrgAdmins` reaches admins and the owner. No partner orgs, no second org.
- Timeouts are not logged (D23): they consume no `id_seq` and do not move the last entry time. A result or event payload that is not JSON gives `Failure::BadOutput` and applies nothing.
- `run_scenario(t, path)` runs the neutral JSON scenario (format in `testing/scenario.rs`; `expect` takes `ok`, `error` (+ exact `fields`), `trap` or `confirmRequired`; a tick always runs as System and refuses `as`; `"save": "name"` stores a step's ok result and an exact `"$name"` string in later `args`/`expect` is replaced by it, so scenarios can name ids an app mints).
- Not yet: actions, `mock_http`, migrations (`upgrade_to`).

## Verify

`cargo test -p calimero-app-sdk` (`tests/harness.rs` runs the harness on the sample guest, `tests/abi.rs` builds `tests/guests/sample`, a standalone crate with its own `[workspace]`, once per run and runs it on the wasmtime engine; `tests/compile_fail.rs` is trybuild over `tests/ui/`), `cargo build -p calimero-app-sdk --target wasm32-wasip2`, `cargo clippy -p calimero-app-sdk --all-targets -- -D warnings` and `cargo clippy -p calimero-app-sdk --target wasm32-wasip2 -- -D warnings`
