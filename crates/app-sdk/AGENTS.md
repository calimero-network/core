# calimero-app-sdk - the Rust app SDK runtime

## Package Identity

- **Crate**: `calimero-app-sdk` (`publish = false`, prototype); builds for the host (tests) and `wasm32-wasip2` (apps)
- **Entry**: `src/lib.rs` (+ `prelude`); `blob.rs` (`BlobRef`, `BlobInfo`), `app.rs` (the `app` attributes from `calimero-app-sdk-macros`, `AppState`, `AppEvent`, `bail!`), `private.rs` (`__private`: the generated `handle`, argument decoding, the ABI assembly; not an API), `encoding/{key,value}.rs`, `store.rs` (+ `store/{mem,host}.rs`), `table.rs`, `commons.rs` (paged reads, counts, clears, checked keys shared by tables and collections), `collections.rs` (+ `collections/{map,counter,cell,set,vector,frozen,per_user,text,rich_text}.rs`), `env.rs` (+ `env/{wasm,mock}.rs`), `guards.rs`, `error.rs`, `json.rs`, `bindings.rs` (wasm32 only), `testing/{mod,directory,scenario}.rs` (feature `testing`, host only)
- **Spec**: `docs/specs/app-runtime-v0.14.md` section 7, the SDK reference, D9, D10, D20, D47, D48, D50, D52

## What This Crate Does

The shared storage encoding (order-preserving tuple keys, deterministic CBOR values), the `Store` trait over the host's ordered key-value storage, tables with indexes, unique constraints and counts, the collections, `env` and the member/admin guards. It re-exports the `app` attributes and `#[derive(Record)]` (see `crates/app-sdk-macros/AGENTS.md`); apps write `use calimero_app_sdk::prelude::*;`. The `testing` feature adds the test harness (host target only).

## Collections

One leader applies every write in log order, so these are plain data structures, not CRDTs (v1's merge semantics have nothing to merge). Every one implements `app::StateField`, so `#[app::state]` lists it in the ABI. Multi-row reads are pages (`FindOpts` in, `Page` out, at most 1,000 rows, `limit + 1` entries read); lengths are stored counts kept with `Store::add`, never a scan; `clear` deletes a batch of at most 1,000 per cursor read until none is left.

| Type (v1 counterpart) | Operations |
| --- | --- |
| `Cell<T>` (`LwwRegister`) | `get`, `set`, `clear` |
| `UnorderedMap<K, V>` (`UnorderedMap`, `SortedMap`, `NestedMap` with tuple keys) | `get`, `contains`, `insert` / `remove` (return the old value), `len`, `is_empty`, `clear`; with `K: FromKey`: `page`, `range`, `prefix` (whole key parts), `starts_with` (last string/bytes part as a prefix), `first`, `last` |
| `Set<T>` (`UnorderedSet`, `SortedSet`), `T: ToKey + FromKey` | `insert`, `remove` (bool), `contains`, `len`, `is_empty`, `clear`, `page`, `range`, `prefix`, `starts_with`, `first`, `last` |
| `Vector<T>` (`Vector`) | `push` (returns the position), `get`, `set` (`OutOfBounds` past the end), `pop`, `len`, `is_empty`, `page` (positions as `KeyPart::Int` in `from`/`to`), `clear` |
| `Frozen<T, H = CborSha256>` (`FrozenStorage`) | `insert` (returns the hex hash and whether it was new; the same content writes nothing), `get(hex)`, `contains(hex)`, `hash_of`; `H: ContentHash<T>` picks the address (default SHA-256 of the stored CBOR) |
| `PerUser<T>` (`UserStorage`) | `get`, `set`, `remove` on the caller's own value (`env::caller()`'s account; `LoginRequired` for non-accounts), `get_for(account)` |
| `Counter` | `get`, `add`, `increment` |
| `Text` (`FugueText`) | `insert_after(id, str)`, `delete_ids`, `insert(index, str)`, `delete(index, n)`, `read`, `chars` (with ids), `id_at`, `index_of`, `len`, `clear` |
| `RichText` (`RichText`) | `text()` / `text_mut()` (a `Text`), `mark(start, end, key, value, expand)`, `mark_ids`, `marks`, `spans`, `clear` |

`Text` stays a sequence CRDT (Tree-Fugue, after v1's `crates/storage/src/collections/fugue.rs`) because editors send edits typed against a stale view: every character has a stable id, an insert names the id it follows and a delete names ids, so edits land where meant and concurrent runs do not interleave. Its reads and edits load every node (a batch of 1,000 per host read); `len` is one read. RGA is not carried over (Fugue supersedes it), nor v1's `RichDocument`, undo, deltas or mark schemas.

## The generated `handle` (`__private::handle`)

- Method lookup by ABI (camelCase) name; unknown → `AppError UnknownMethod`.
- Kind: a view may run as a call (or xcall delivery); a call, timer or init method run as a view → `NotAView`; any other mismatch, and `action` / `migrate` (not in this version) → `WrongKind`.
- Callers (D13), checked after the kind, except for `init` (runs as the owner): `System` needs `allow(system)` and `App(id)` needs `allow(app = id)`, else `Forbidden`; `Public` needs `public`, else `LoginRequired`. The host's checks come first (Rulings 32, 35): on a node, and in the harness, an anonymous caller reaches the guest only on a public app and only for a `public` method, otherwise the platform answers `401 UNAUTHENTICATED` before `handle` runs, so the guest-side `LoginRequired` is a backstop clients do not normally see.
- Args: a JSON object keyed by camelCase parameter names, empty args = `{}`, unknown keys refused; `u64`/`i64` (and `Option`) params are canonical decimal strings (no `+`, no leading zeros, no `-0`) and `Vec<u8>` (and `Option`) params base64url without padding (D20); anything malformed → `BadArgs` (`{"message": ..}`).
- Timers: `#[app::every]` implies `allow(system)` (Ruling 26), so the ABI says `allowSystem: true` on every timer.
- Timer ticks: for `kind = timer` the engine passes the tick as `args`, `{"scheduled":"<ms>","previous":"<ms>"|null}`; `handle` sets `env::timer()` from it (and clears it for every other kind).
- Results: JSON; a `u64`/`i64` (or `Option`) result is a decimal string, a `Vec<u8>` (or `Option`) result base64url; `()` and `init` give `null`. Records inside results: their `u64`/`i64` fields are decimal strings and `Vec<u8>` fields base64url too (`json::*_auto`).

## D20 conversion rule (Ruling 27)

The macros convert exactly these types, at the top level of a parameter, a result, a record field, or an event or error field: `u64`, `i64`, `Vec<u8>` and `Option` of each. The same types nested inside another type the macro can see (`Vec<u64>`, `Option<Vec<i64>>`, `Option<Option<u64>>`, tuples, arrays, maps, `Vec<Vec<u8>>`) are a compile error naming D20 ("u64 inside Vec<u64> would lose precision in JSON (D20); wrap it in a Record or use a string"). Limit: the macro sees only the tokens it is given. A user struct with plain serde, or a `type` alias, is opaque to it, so a `u64` inside one still reaches JSON as a number; use a `Record` (its derive converts the fields) or a string.
- Errors: the wire `app-error` is `{ code, message }` with `message` = the fields as JSON (`{}` when none), e.g. `Taken` / `{"start":"7"}`.

## Rules

- Key layouts are the cross-language contract (the TypeScript SDK must produce the same bytes): change them only with the spec.
- Table counts, `Counter::add` and every collection length use `Store::add`, never read-modify-write (D47).
- Keys over 1 KiB are refused with `StorageError::KeyTooLong` before any store call (D50).
- Queries read `limit + 1` entries through the cursor and no more.
- `bindings.rs` declares the `handle` export but never exports it: the code `#[app::logic]` generates does, through `__private::bindings::__calimero_export_handle!` and `__private::Export<State>`.
- `#[app::error]` and `#[app::event]` must live in the `#[app::logic]` module (their ABI constants are picked up there; elsewhere the ABI lists none), and one module takes one of each: a second `#[app::error]` enum in a module is refused (`__CALIMERO_ABI_ERRORS` defined twice, pinned by `tests/ui/second_error.rs`).
- `env::emit(event, audience)` takes an `#[app::event]` value (spec); `env::emit_raw(name, data, audience)` takes any serde value.
- `env::mock` (host target) is the stand-in for the host in tests.

## Encoding contract (the TypeScript SDK must match)

- **Keys**: tag byte per part (none 0x00, bool 0x10, int 0x20, string 0x30, bytes 0x40); ints are sign-flipped big-endian 16 bytes; strings and bytes escape `0x00` as `0x00 0xFF` and end with `0x00 0x00`.
- **Collection keys**: every key starts with the collection's name (the state field in camelCase) as a string part, then a kind part: `UnorderedMap` `(name, "m", key..)`, `Set` `(name, "s", element..)` holding CBOR `null`, `Vector` `(name, "e", position)`, `Cell` `(name, "v")`, `Frozen` `(name, "f", hash as 32 bytes)`, `PerUser` `(name, "u", account)`, `Counter` `(name, "n")`; the map, set and vector keep their length at `(name, "n")`. `Text` keeps a character at `(name, "t", id)` as `{ch, parent, right}` (`ch` null once deleted), its live length at `(name, "n")` and the next id at `(name, "i")`; `RichText` adds marks at `(name, "k", id)` as `{key, value, start, end, endInclusive}`, ids from the same `(name, "i")`.
- **Key length (D50, ruling 24)**: the host refuses keys over 1,024 bytes. The SDK caps every key it stores (record, index, unique, count, map, counter) at 1,023 (`MAX_SDK_KEY_BYTES`), so derived bounds (`last ++ 0x00`, `prefix ++ 0xFF`) always fit. Every range bound and decoded cursor is checked before any store call: bounds give `KeyTooLong`, a cursor gives `BadCursor`.
- **Unique constraints use SQL NULL semantics (ruling 23)**: when any field of a unique constraint is `None`, no unique entry is written or checked (insert, upsert, update, remove). Two records with no email both insert; the same `Some(email)` still clashes.
- **Values**: deterministic CBOR (RFC 8949 core). Field names are camelCase (applied by the `#[app::state]`/record macro, not by the SDK). `None` is written as CBOR null, never omitted. An integral float stays a float (`1.0` is `f93c00`), floats use the shortest exact form, `-0.0` is preserved, NaN is refused, map keys sort by encoded bytes, lengths are definite.
- **JSON (D20, ruling 25)**: `u64`/`i64` in method args and results, event data and `AppError` fields are canonical decimal strings; bytes are base64url without padding; use `calimero_app_sdk::json::{u64_str, i64_str, opt_u64_str, opt_i64_str, bytes_b64url, opt_bytes_b64url}` (the macros apply them). Record fields use `json::{u64_auto, i64_auto, bytes_auto, ..}`: an integer or a byte string (major type 2) in CBOR (not human-readable), a decimal string or base64url in JSON.
- **Blob references (R8)**: a `BlobRef` is CBOR tag 51,739 (`0xCA1B`) around the blob's 32-byte SHA-256 in stored values (the host finds them and keeps the blob while one exists), and the 64-lowercase-hex id in JSON. `BlobRef::info()` / `grant(ttl)` (`env::blob_info` / `blob_grant`) reach the WIT `blobs` interface.
- **Cursors** are the base64url of the last storage key, bound to the index and prefix, not signed.
- **Prefixes** longer than the index or count fields are refused (`PrefixTooLong`); `limit: 0` is an empty page.

## Test harness (`testing` feature, spec section 10)

- `TestApp::new(component, Org, now_ms)` prepares the component on the real `WasmtimeEngine` (one engine and compile cache per test process) and runs `init` as `"owner"`. `t.as_(a)` / `anonymous()` / `system()` / `api_key(id, account)` give an `As` with `call`, `view`, `confirmed()`; `Failure` is `App { code, fields }`, `Trap(TrapKind)`, `ConfirmRequired`, `BadOutput` or `Refused { status, code }`.
- The harness makes the node's checks before the app runs, with the node's codes (Ruling 32; the node is the reference). Per access kind, as on a node: an org or owner app refuses every anonymous caller with `401 UNAUTHENTICATED` at the access gate, before the ABI is read, `public` methods included; a public app lets anonymous callers reach the ABI, where a method that is not `public` is `401 UNAUTHENTICATED`. The harness app is an org app, so anonymous is always `Refused { 401, "UNAUTHENTICATED" }`, and an account the org does not hold (never in it, or removed) is `Refused { 404, "NOT_FOUND" }` (which does not confirm the app exists). A guest reaches the app, whose own guard decides (`App { "Forbidden" }`). Then call arguments over the node's 512 KiB (`Refused { 413, "ARGS_TOO_LARGE" }`, Ruling 30). Then the ABI's checks, in the node's order and with its codes (`node::abi_check`): `UnknownMethod` (also any reserved `__` name), `WrongKind` for `init` and timers (as a call or a view), `ConfirmNotAView`, `NotAView`, `Forbidden` for System without `allowSystem`, `ConfirmRequired`; pinned by `rooms.json`, which the cluster runs too. Typed client errors such as `LoginRequired` belong in the client SDK, mapped from the HTTP status in one place (not this milestone).
- It plays the executor: the diff is applied only on `Ok`; each logged invocation gets a fresh `id_seq` (`id_term` 1) and a seed derived from `t.seed(n)`; views see the last logged entry's timestamp (D19). Before running it applies the ABI checks a node makes: a `confirm` method needs `.confirmed()` (else `ConfirmRequired`) and is never a view (`ConfirmNotAView`); a method without `allowSystem` refuses `system()` (`Forbidden`). Anonymous callers never get this far (the access gate above).
- `t.tick(method)` runs the timer as `System` for the slot at the current time; `previous` is the last tick of the same method. A tick that does not time out counts as applied (D34).
- Events: `events_for(account)` resolves audiences when asked (R9); `remove_member` first fixes who org events so far reached, so earlier events stay with the leaver and later ones do not reach it. 
- `export_state()` is deterministic CBOR (RFC 8949 4.2.1): a map of stored key (generation prefix included) to value, byte strings, ordered by encoded key bytes. The TypeScript harness must produce the same bytes.
- What the harness org supports: one org (`ORG_ID`) with the owner (`"owner"`, role Owner), `admins`, `members`, `guests` (role checks, `org_role`, `orgs_of`), and `Org::teams([("id", ["acct", ..])])` (`in_team`, `team_exists`, `Team` audiences; a team needs no org role). `Org` audiences reach members, admins and the owner, not guests; `OrgAdmins` reaches admins and the owner. No partner orgs, no second org.
- Timeouts are not logged (D23): they consume no `id_seq` and do not move the last entry time. A result or event payload that is not JSON gives `Failure::BadOutput` and applies nothing.
- `run_scenario(t, path)` runs the neutral JSON scenario (format in `testing/scenario.rs`; `expect` takes `ok`, `error` (+ exact `fields`), `trap`, `confirmRequired` or `refused` (the node's API code of a refusal before the app ran, e.g. `"UNAUTHENTICATED"`, `"NOT_FOUND"`); a tick always runs as System and refuses `as`; `"save": "name"` stores a step's ok result and a string that is exactly `$` plus a name (`[A-Za-z_][A-Za-z0-9_]*`) in later `args`/`expect` is replaced by it (other strings, like `"$5 lunch"`, stay literal; `save` on tick/advance/removeMember is an error), so scenarios can name ids an app mints; a string that is exactly `@` plus a name in `args`/`expect` is that account as the app sees it: its name here, its account id on a cluster (`crates/node/tests/rooms.rs` runs the same files)).
- Size limits: writes and events past 1 MiB per run are refused inside the run by the same `HostCore` the node uses (`MAX_RUN_BYTES`); the node's final check of the encoded log entry (JSON, values base64) is not mirrored, so an entry within `MAX_RUN_BYTES` whose encoding passes 1 MiB passes here and traps `HostDenied` on a node.
- Blobs: `t.upload_blob(bytes, mime)` records a blob as the node would on upload and returns its id; runs see it through `info`/`grant` (a grant is `"<id>.<expires_ms>"` here) and may store a `BlobRef` to it; `t.blob_refs(id)` counts the stored values referring to it. Idle expiry is the node's (not mirrored).
- Not yet: actions, `mock_http`, migrations (`upgrade_to`).

## Verify

`cargo test -p calimero-app-sdk` (`tests/harness.rs` runs the harness on the sample guest, `tests/abi.rs` builds `tests/guests/sample`, a standalone crate with its own `[workspace]`, once per run and runs it on the wasmtime engine; `tests/compile_fail.rs` is trybuild over `tests/ui/`), `cargo build -p calimero-app-sdk --target wasm32-wasip2`, `cargo clippy -p calimero-app-sdk --all-targets -- -D warnings` and `cargo clippy -p calimero-app-sdk --target wasm32-wasip2 -- -D warnings`
