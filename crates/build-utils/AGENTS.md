# calimero-build-utils - Build Script Helpers

Shared helpers for `build.rs` scripts: reads the workspace release version and stamps git/rustc metadata into `cargo:rustc-env` vars, and behind the optional `fetch` feature downloads and extracts zip archives into a build cache.

## Package Identity

- **Crate**: `calimero-build-utils`
- **Entry**: `src/lib.rs`, plus `src/fetch.rs` behind the `fetch` feature
- **Key deps**: `rustc_version` (compiler version string), `toml` (parse workspace `Cargo.toml`); `fetch` only: `eyre`, `reqwest`, `sha2`, `tempfile`, `zip`

## Commands

```bash
# Build
cargo build -p calimero-build-utils

# Test (all, including the fetch helpers)
cargo test -p calimero-build-utils --features fetch

# Test a single case
cargo test -p calimero-build-utils git_details_or_unknown_returns_unknown_outside_git_repo -- --nocapture
```

## Helper Inventory

| Item | Kind | Purpose |
| --- | --- | --- |
| `set_version_env_vars(prefix)` | fn | The one-call entry point for a `build.rs`: sets `<PREFIX>_VERSION`, `<PREFIX>_BUILD`, `<PREFIX>_COMMIT`, `<PREFIX>_RUSTC_VERSION` |
| `read_workspace_version()` | fn | Walks up from `CARGO_MANIFEST_DIR` to find `[workspace.metadata.workspaces].version` in a `Cargo.toml`; emits `cargo:rerun-if-changed` on the file it found; `None` if not found |
| `git_details(pkg_dir)` | fn | Runs `git describe --always --dirty=-modified --tags --match [0-9]*` and `git rev-parse --short HEAD` from `pkg_dir`; emits `cargo:rerun-if-changed` for `HEAD`/`logs/HEAD`/`index`; returns `Result<GitInfo, _>` |
| `git_details_or_unknown(pkg_dir)` | fn | Same as above but swallows errors into `GitInfo { describe: "unknown", commit: "unknown" }` and emits `cargo:warning` instead of failing the build |
| `GitInfo` | struct | `{ describe: String, commit: String }` |
| `run_command(cmd, args, cwd)` | fn | Thin wrapper over `std::process::Command`; returns stdout as `String`, error includes stderr on non-zero exit |
| `fetch_and_extract(client, src, cache_dir, freshness, force)` | fn | `fetch` feature. Downloads (or reads, for a local path) a zip and returns the directory it was extracted into; reuses a cached extraction younger than `freshness` unless `force` |

`read_workspace_version_for_dir` and `parse_workspace_metadata_version` are private helpers used only by `read_workspace_version` and the test suite.

## Mental Model

`set_version_env_vars` is called by the `merod` and `meroctl` binaries (`crates/merod/build.rs`, `crates/meroctl/build.rs`) as `calimero_build_utils::set_version_env_vars("MEROD")` / `"MEROCTL"`, `.expect()`-ing the result - a build.rs failing hard here is intentional, not a bug to soften. `fetch_and_extract` is called by `crates/server/build.rs` and `crates/auth/build.rs`, which enable the `fetch` feature to pull the admin dashboard and auth frontend bundles.

`set_version_env_vars` composes the other three: it reads the workspace release version (from the workspace-root `[workspace.metadata.workspaces].version`, not the placeholder `0.0.0` in each crate's own `Cargo.toml`), resolves git info relative to `CARGO_MANIFEST_DIR` (falling back to `"unknown"` rather than failing if the crate is built outside a git checkout, e.g. from a source tarball), and reads the active `rustc` version. It then prints four `cargo:rustc-env=...` lines, which downstream code reads via `env!(...)` at compile time (see `crates/merod/src/version.rs`, `crates/merod/src/cli.rs`, `crates/meroctl/src/version.rs`, `crates/meroctl/src/cli.rs`).

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Version/git helpers, `GitInfo`, and their tests |
| `src/fetch.rs` | `fetch_and_extract` and its cache, behind the `fetch` feature |

## Gotchas

- `git_details` resolves `git rev-parse --git-dir` and joins it onto `pkg_dir` before reading `HEAD`/`logs/HEAD`/`index` for `rerun-if-changed` - this is deliberate because `--git-dir` can print a relative path, and getting this wrong means cargo caches a stale build across git commits instead of re-running.
- `read_workspace_version` walks *up* the directory tree from `CARGO_MANIFEST_DIR` looking for a `Cargo.toml` with `[workspace.metadata.workspaces].version` - it will find the first ancestor `Cargo.toml` that has that key, so a crate must actually be inside the workspace for this to resolve; outside a workspace it returns `None` and `set_version_env_vars` turns that into a hard `Err`.
- `git_details_or_unknown` exists specifically so binaries still build (with `"unknown"` version metadata) when packaged/built outside a git repo; `git_details` itself is strict and returns `Err` in that case.
- The workspace version lives in the root `Cargo.toml` under `[workspace.metadata.workspaces]`, separate from `[workspace.package].version` (which stays `"0.0.0"`, managed by `cargo-workspaces` per `docs/RELEASE.md`) - don't confuse the two when debugging a wrong version string.

Part of [crates/](../AGENTS.md).
