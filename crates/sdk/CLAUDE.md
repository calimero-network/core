# calimero-sdk - App Development SDK

SDK for developing Calimero WebAssembly applications. Provides macros, CRDT helpers, and environment access.

- **Crate**: `calimero-sdk`
- **Entry**: `src/lib.rs`
- **Sub-crates**: `calimero-sdk-macros`, `calimero-sdk-near`

## Build & Test

```bash
cargo build -p calimero-sdk
cargo test -p calimero-sdk
cargo test -p calimero-sdk-macros     # compile-time macro tests (trybuild)
cargo build -p kv-store --target wasm32-unknown-unknown --release
```

## File Layout

```
src/
├── lib.rs              # Public exports
├── state.rs            # State management
├── env.rs              # Environment access
├── env/ext.rs          # External (host) functions
├── event.rs            # Event handling
├── returns.rs          # Return types
├── types.rs            # SDK types
├── macros.rs           # Re-exported macros
└── private_storage.rs  # Private storage utilities
macros/src/
├── lib.rs              # Proc macro entry
├── app.rs              # #[app::*] macro implementation
└── state.rs            # State macro
macros/tests/
├── *.rs                # Trybuild tests
└── *.stderr            # Expected compile errors
libs/near/              # NEAR-specific extensions
```

## Core Macros

### `#[app::state]`

Declares the application state struct. Required on exactly one struct per app.

```rust
#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct AppState {
    items: UnorderedMap<String, LwwRegister<String>>,
}
```

### `#[app::logic]`

Marks an impl block as the app entry point. Methods become callable via JSON-RPC.

```rust
#[app::logic]
impl AppState {
    #[app::init]               // required: constructs initial state
    pub fn init() -> AppState { ... }

    pub fn mutate(&mut self, ...) -> app::Result<()> { ... }   // &mut self = mutation
    pub fn query(&self, ...) -> app::Result<T> { ... }         // &self = read-only
}
```

### `#[app::event]`

```rust
#[app::event]
pub enum Event<'a> {
    Inserted { key: &'a str, value: &'a str },
    Updated  { key: &'a str, value: &'a str },
}
```

## Macros

| Macro | Purpose |
|---|---|
| `app::emit!(Event::Variant { ... })` | Emit an event |
| `app::log!("msg: {:?}", val)` | Log a message |
| `app::bail!(Error::Variant(...))` | Return an error early |

## Environment Access

```rust
let context_id = app::context_id();   // current ContextId
let executor    = app::executor_id(); // caller's public key
```

## Error Handling Pattern

```rust
use calimero_sdk::serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("key not found: {0}")]
    NotFound(&'a str),
}

pub fn get_required(&self, key: &str) -> app::Result<String> {
    let Some(val) = self.get(key)? else {
        app::bail!(Error::NotFound(key));
    };
    Ok(val)
}
```

## Key Files

| File | Purpose |
|---|---|
| `src/lib.rs` | Public API re-exports |
| `macros/src/app.rs` | `#[app::*]` macro implementation |
| `src/env.rs` | `context_id()`, `executor_id()` |
| `src/event.rs` | Event emission |
| `apps/kv-store/src/lib.rs` | Best reference app |

## Quick Search

```bash
rg -n "pub fn " macros/src/
rg -n "pub " src/lib.rs
rg -l "#\[app::state\]" ../apps/
rg -n "app::emit!" ../apps/
rg -n "app::bail!" ../apps/
ls macros/tests/*.stderr
```

## Gotchas

- `#[app::init]` is required — without it the app panics on first call
- State struct needs both `BorshSerialize` + `BorshDeserialize`
- Use `calimero_sdk::borsh` / `calimero_sdk::serde` crate paths in derives, not bare `borsh`/`serde`
- Errors must impl `Serialize` for JSON-RPC to return structured errors
- Methods returning `app::Result<T>` are exposed as RPC; others are private
