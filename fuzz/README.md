# Fuzz targets

Coverage-guided fuzzing of the code a node runs on bytes a peer controls.
Each target calls the production entry point and asserts an invariant beyond "no panic".

```bash
scripts/fuzz.sh <target> [seconds]
```

The script installs nightly and `cargo-fuzz` if they are missing, and runs until a crash, or for `seconds`.
New inputs go to `fuzz/corpus/<target>/`, a crash to `fuzz/artifacts/<target>/`; neither is committed.
After one script run has copied the root lock in, replay a crash with `cargo +nightly fuzz run <target> <file>` from this directory.

| Target | Entry point | Invariant |
| --- | --- | --- |
| `bundle_open` | `VerifiedBundle::open`, then artifact extraction | an artifact is a verbatim regular-file slice of the archive; a read manifest is a recognised bundle |
| `crdt_sync` | `Root::sync` of a peer delta over a `RichText`, wrapped as the node wraps it | after a sync that succeeds, the text and formatting read back, a refused insert changes nothing, and the local replica can still format |

Private entry points are reached through `fuzz_api` modules compiled only under `cfg(fuzzing)`; they wrap the real function and hold no logic.
Seeds live in `fuzz/seeds/<target>/`, including the reproduction input of the finding each target guards.
