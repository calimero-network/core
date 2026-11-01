# calimero-app-sdk-macros - the `app` attributes

## Package Identity

- **Crate**: `calimero-app-sdk-macros` (`publish = false`, prototype; proc-macro); re-exported by `calimero-app-sdk` as `calimero_app_sdk::app::*` and `calimero_app_sdk::Record`. Apps never depend on it directly.
- **Entry**: `src/lib.rs`; `state.rs` (`#[app::state]`), `logic.rs` (`#[app::logic]`, the dispatcher and the ABI), `payload.rs` (`#[app::error]`, `#[app::event]`), `record.rs` (`#[derive(Record)]` + `#[table]`), `types.rs` (camelCase, ABI type names, 64-bit integer detection)
- **Spec**: `docs/specs/app-runtime-v0.14.md` SDK reference ("Defining an app", "Errors", "Numbers across languages", "Actions, timers and migrations"), section 4 host rules, D13, D20, D48, D51

## What This Crate Does

- `#[app::state(version = N)]` implements `app::AppState`: `load()` builds each field with `<FieldType>::new("<fieldInCamelCase>")` (state holds only handles, so loading reads nothing); `ABI` is `{"fields":{"<fieldInCamelCase>":"<kind>",..},"search":{"<fieldInCamelCase>":[..],..},"version":N}`, each kind the field type's `app::StateField::KIND` (`table`, `unorderedMap`, `counter`, `cell`, `set`, `vector`, `frozen`, `perUser`, `text`, `richText`) and each search list its `StateField::SEARCH` (a table's `#[table(search = [..])]`, else `[]`), joined at compile time.
- `#[app::logic]` on `impl State { .. }` exposes every `pub fn` plus the `#[app::init]` one (private fns stay helpers), strips the method attributes (`init`, `view`, `confirm`, `public(rate = ..)`, `every("<n>s|m|h")`, `allow(system)`, `allow(app = "..")`, `"*"` for any app the node delivers from), implements `__private::Dispatch` (one serde args struct per method, `deny_unknown_fields`), emits the `handle` export (wasm32 only) and the ABI as a `#[link_section = "calimero:abi"]` static assembled at compile time from `<State as AppState>::ABI` and the module's `__CALIMERO_ABI_ERRORS` / `__CALIMERO_ABI_EVENTS`.
- Compile-time refusals: `every` intervals other than `<n>s|m|h`, `public(rate = ..)` other than `<n>/s|min|h` (n > 0), `init` with any other attribute (`confirm`, `public`, `allow(system)`, `allow(app = ..)`), a `u64`/`i64`/`Vec<u8>` nested in another type (Ruling 27, see `crates/app-sdk/AGENTS.md`), `partition`. `every` implies `allow(system)` (Ruling 26).
- `#[app::error]` implements `From<E> for AppError` (code = variant name); `#[app::event]` implements `app::AppEvent` (name = variant name). Fields: camelCase keys, tuple fields `"0"`, `"1"`; `u64`/`i64` (and `Option` of them) as decimal strings (D20, Ruling 25).
- `#[derive(Record)]` implements `Record` (camelCase index/count/unique names and field lists, checked against the struct; `search = ["field", ..]` sets `SEARCH` and `SEARCH_JSON`) and serde: camelCase names; `u64`/`i64` fields are CBOR integers when stored and decimal strings in JSON results (`json::*_auto`, via `is_human_readable`). `partition` is refused: "partition is reserved (D48)". The derive writes the serde impls, so a record must not also derive `Serialize`/`Deserialize` (its field types still do).

## ABI contract (read by `calimero_app_host::Abi::from_json`)

Every object's keys are written in sorted order (`types::canonical`, never `serde_json::Map` order), so the bytes do not depend on serde_json's `preserve_order`, which the core workspace unifies in through dcap-qvl; the unit tests turn that feature on to prove it.

`{"abiVersion":1,"world":"calimero:app@0.5.0","state":{"fields":{field:kind},"search":{field:[textField,..]},"version":N},"methods":{name:{"kind":"init|call|view|timer","params":[[name,type]],"returns":type|null,"confirm","public","allowSystem"[,"every"][,"rate"][,"allowApps"]}},"errors":{code:{field:type}},"events":{name:{field:type}}}`. `every` appears on timers only (the host refuses it elsewhere). Types: Rust integer/float/bool names, `string`, `bytes`, `option<T>`, `list<T>`, `tuple<..>`, `unit`, else the Rust type name.

## Rules

- Generated code names only `::calimero_app_sdk::..` paths (`__private` for support code, `__private::serde` via `#[serde(crate = ..)]`); an app depends on the SDK and nothing else from us.
- `#[app::error]` / `#[app::event]` must be in the `#[app::logic]` module: their ABI constants shadow the glob-imported `__private::abi_defaults` there. Elsewhere, the ABI silently lists none. A second enum of either kind in one module is refused (the constant is defined twice).
- A field the host enforces needs an `abiVersion` bump (D51); free-form keys (`params`, `returns`, `errors`, `events`) are ignored by nodes.
- Not in this version: `#[app::action]`, `#[app::migrate]`, `#[app::on_*]`, `allow(action)`, typed `Index::..` enums (indexes are named by camelCase strings).

## Verify

`cargo test -p calimero-app-sdk-macros` (unit) and `cargo test -p calimero-app-sdk --test abi --test compile_fail` (the sample guest end to end, the D48 refusal); `cargo clippy -p calimero-app-sdk-macros --all-targets -- -D warnings`
