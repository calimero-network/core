# calimero-app-sdk - the old name of calimero-sdk

## Package Identity

- **Crate**: `calimero-app-sdk` (`publish = false`); `src/lib.rs` only.
- **Is**: `pub use calimero_sdk::*;`, and a `testing` feature that turns on `calimero-sdk/testing`.

## What This Crate Does

Keeps apps that still depend on `calimero-app-sdk` building while they move to
`calimero-sdk` (see `crates/sdk/AGENTS.md`). The `app` attributes in
`calimero-sdk-macros` find the SDK by the name the app's manifest lists it under
(`calimero-sdk` first, then `calimero-app-sdk`, or a renamed dependency), so
generated code resolves through this re-export.

## Rules

- Add nothing here: every type, macro and feature lives in `calimero-sdk`.
- Delete this crate, and `calimero-app-sdk` from `calimero-sdk-macros`' lookup,
  once no app in this repository or calimero-network/apps depends on it.
