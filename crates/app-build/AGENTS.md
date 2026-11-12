# calimero-app-build - Rust guest to `calimero:app` component

## Package Identity

- **Crate**: `calimero-app-build` (`publish = false`, prototype; the binary ships with each release instead); lib plus bin `mero-app-build`; the standard library's `File::lock` for the build lock
- **Entry**: `src/lib.rs`, `src/main.rs`
- **Spec**: `docs/specs/app-runtime-v0.14.md` (D45 build path)

## What This Crate Does

`build_components(crate_dir, target_dir)` builds the app component and, when its ABI has compute methods, the compute component (`build_compute_component`: the same crate under `CARGO_ENCODED_RUSTFLAGS` plus `--cfg calimero_compute`, into `<target>/compute`, so the two builds never invalidate each other); `mero-app-build <crate> [--out <file>] [--compute-out <file>] [--target-dir <dir>]` writes both (the compute one beside `--out` as `compute.component.wasm` by default). `build_component(crate_dir, target_dir)` runs `cargo build --release --target wasm32-wasip2`, lifts the `calimero:abi` custom section from the nested core module to the component's top level, and returns the component, its ABI JSON and its top-level imports. `build_component` holds an exclusive file lock (`<target>/.calimero-app-build.lock`) for the build, so threads and processes sharing a target dir (nextest) build one at a time; test: `tests/concurrent_build.rs`. `abi_of` and `imports_of` read a component's top level only.

`build_components_cached(crate_dir, also_hash, cache_dir)` reuses `cache_dir/<name>-<cache_key>.wasm` (the app component) and `<name>-<cache_key>.compute.wasm` (the compute one, written first and present only when the app has compute methods), and returns both as `CachedApp`.
`build_component_cached` returns the app component of the same entry.
A miss builds under an exclusive cross-process file lock (`std::fs::File::lock`, `cache_dir/target/<name>.build.lock`, one per crate and target dir; Ruling 46), recomputes the key and re-checks the cache once it holds the lock, so concurrent processes (nextest runs each test in its own) build once and the rest reuse the result. The key walk skips a file that vanishes while it walks. Test: `tests/concurrent_cache.rs` (6 processes on a cold cache, one `cargo build`).

`mero-app-build codegen --abi <abi.json|.wasm|.mpk|manifest.json> --lang ts|python|swift|kotlin|openapi [-o <dir>] [--name <n>] [--service <s>] [--package <p>] [--kotlin-serialization-plugin] [--no-openapi]` writes the app's `openapi.json` (unless `--no-openapi`) and a typed client (`client.ts` over mero-js, `client.py` over calimero-client-py, `<Name>Client.swift` / `.kt`). The library runs the same in process: `codegen::Codegen::new(abi, lang, out)` and `run()`, which returns the paths written (`src/codegen.rs`). The client's name is `--name`, else the service, else the bundle's package, else the file's stem; a stem that names no app (`abi`, `app`, `component`, `compute`, `manifest`) falls back to the package name of the nearest `Cargo.toml` above the file, and with none the run fails asking for `--name`. `mero-app-build abi-diff <old> <new> [--json] [--service <s>]` prints what changed and exits 1 on a breaking change. The CLI side of both is `src/abi_tools.rs` over `calimero-app-abi`; test: `tests/abi_tools.rs`. A crate directory named `codegen` or `abi-diff` is passed as `./codegen`. `mero-app-build --version` (`-V`) prints the release (`build.rs` reads `[workspace.metadata.workspaces].version`) and what it was built from, `(<git describe>, <commit>)` (`unknown` outside git). `--help` / `-h` (top level, `codegen`, `abi-diff`) prints the usage and exits 0.

## Rules

- Section lifting is raw byte surgery on section boundaries (no re-encode of contents); keep it that way.
- Table `partition` refusal is not here; it is a compile error in the `#[table]` derive.
- `tests/guests/echo` is a standalone crate (own `[workspace]`), not a member of the core workspace.

## Verify

`cargo test -p calimero-app-build`

Put `mero-app-build` on PATH with `scripts/install-mero-app-build.sh`; every doc, script and CI job calls it by that name. Releases ship it prebuilt (`mero-app-build_<target>.tar.gz`, Linux x86_64/aarch64 and macOS aarch64, in `SHA256SUMS`) for apps that only run `codegen`; the crate stays `publish = false`, as does `calimero-app-abi`.
