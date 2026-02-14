# Versioning and release

This document describes how versions are managed and how to cut a release. The approach follows [NEAR’s nearcore](https://github.com/near/nearcore) layout.

## Where the version lives

- **`[workspace.metadata.workspaces].version`** in the root **`Cargo.toml`** is the single source of truth for the release version. The release workflow and tooling read this value (e.g. via `cargo metadata | jq -r '.metadata.workspaces.version'`). All published crates use this version when you run `cargo ws publish`.

- **`[workspace.package].version`** is kept at `"0.0.0"` and is a placeholder. It is not updated by the workflow; `cargo-workspaces` uses the metadata version when publishing.

## How to release a new version

1. **Bump the version**  
   In the root **`Cargo.toml`**, under `[workspace.metadata.workspaces]`, set:
   ```toml
   version = "1.2.3"   # or e.g. "0.10.0-rc.43"
   ```
   Commit and merge (e.g. via a “release” or “bump version” PR).

2. **Let the release workflow run**  
   On the appropriate trigger (e.g. push to `master`), the workflow will:
   - Read the version from `cargo metadata` (i.e. from `[workspace.metadata.workspaces].version`).
   - Build binaries, publish crates with `cargo ws publish`, and create the GitHub release using that version. No `sed` or in-repo replacement is used; the version in `Cargo.toml` is used as-is.

## Binaries vs libraries

- **Binaries (merod, meroctl)** get their displayed version from **build-time env vars** set in each binary’s `build.rs` (e.g. `MEROD_VERSION`, `MEROCTL_VERSION` from `CARGO_PKG_VERSION` plus git describe/commit). See [NEAR’s neard](https://github.com/near/nearcore/blob/master/neard/src/main.rs).

- **Libraries** that need a “current” version (e.g. `calimero-node-primitives` for bundle `minRuntimeVersion` checks) use **`CARGO_PKG_VERSION`** in their own `build.rs` and `env!("...")` in code, so they stay in sync with the workspace version.

- The **`Version`** type and protocol-related version types live in **`calimero_primitives::version`** (similar to `near_primitives::version`).

## References

- [nearcore Cargo.toml](https://github.com/near/nearcore/blob/master/Cargo.toml) – `[workspace.package]` and `[workspace.metadata.workspaces]`
- [near_crates_publish.yml](https://github.com/near/nearcore/blob/master/.github/workflows/near_crates_publish.yml) – version from metadata, no `sed`
