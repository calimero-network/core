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

Private entry points are reached through `fuzz_api` modules compiled only under `cfg(fuzzing)`; they wrap the real function and hold no logic.
Hand-made seeds live in `fuzz/seeds/<target>/`, including the reproduction input of the finding each target guards.
