# calimero-git-hooks - Self-Installing Git Hooks

Dev-only build script that installs the repo's `.githooks/` scripts into the current checkout, including linked worktrees.

## Package Identity

- **Crate**: `calimero-git-hooks`
- **Entry**: `build.rs` (the crate's real content); `src/lib.rs` is intentionally empty - it exists only so the build script runs as part of a normal `cargo build`/`cargo test`
- **Key deps**: none beyond `std` (`std::process::Command` to shell out to `git`, `std::fs`)

## Commands

```bash
# Build (this alone installs/refreshes the hook)
cargo build -p calimero-git-hooks

# Test (also triggers the build script)
cargo test -p calimero-git-hooks
```

There is nothing to unit-test in this crate - no `#[test]` functions exist. Verification is behavioral: build it, then check the hook landed (see below).

## Mental Model

`build.rs::main` runs on every `cargo build`/`cargo test` (any crate that depends on this one, or the workspace build itself, triggers it) and:

1. Resolves the working-tree root via `git -C <manifest_dir> rev-parse --show-toplevel`. If that fails (no git, published crate/tarball with no `.git`), it's a silent no-op.
2. Reads scripts from `<toplevel>/.githooks/` - the source of truth. Registers `cargo:rerun-if-changed` on that directory and each file in it, so edits to `.githooks/pre-commit` are picked up on the next build.
3. Asks git itself where hooks belong for *this* checkout via `git rev-parse --git-path hooks`, rather than assuming `<toplevel>/.git/hooks`. That call resolves correctly for a plain clone, a linked worktree (where `.git` is a file and hooks live in the shared common dir), and any `core.hooksPath` override.
4. Copies each file from `.githooks/` into that resolved hooks directory, inserting the marker `# managed-by: calimero-git-hooks` right after the shebang line, and `chmod`s it `0o755` on Unix.

Currently `.githooks/` holds one script, `pre-commit`, which runs `cargo fmt --check` against staged files matching `\.rs$` (via `git diff --cached --name-only --diff-filter=ACMR`), mirroring the fmt gate in CI.

## Key Files

| Path | What's there |
| --- | --- |
| `build.rs` | All logic: `git()` helper, marker handling, worktree-aware hooks-dir resolution, install loop |
| `src/lib.rs` | Empty - just a doc comment explaining why the crate exists |
| `.githooks/pre-commit` (repo root) | The actual hook script (`cargo fmt --check` gate); this is what gets installed, not code in this crate |

## Invariants and Gotchas

- **Never clobber a foreign hook**: before overwriting an existing file at the destination, the installer checks it contains the `MARKER` string. If it doesn't, it logs `cargo:warning=... leaving ... untouched` and skips - a contributor's own custom hook, or one installed by another tool, is left alone.
- **Idempotent**: if the destination already matches the freshly-rendered content (marker inserted, byte-for-byte), it's skipped rather than rewritten - no spurious mtime churn on every build.
- **Everything is a quiet no-op on failure**: missing `git` binary, not a repo, missing `.githooks/`, or a read-only hooks dir all just `return` early (with a `cargo:warning` where a write was attempted and failed). The build script must never fail the build over hook installation.
- **Worktree support is the whole point of the `git rev-parse --git-path hooks` call** - do not replace it with a hardcoded `<toplevel>/.git/hooks` path, that breaks for linked worktrees where hooks live in the shared common `.git` dir.
- **`.githooks/` is the only place to edit hook logic.** Editing the installed copy under `.git/hooks/` (or the worktree's common dir) is pointless - the next `cargo build`/`cargo test` overwrites it from `.githooks/` again.

Part of [crates/](../AGENTS.md).
