# mero-py: the Python client

The Python client for a Calimero node: a PyO3 extension over core's Rust
`calimero-client` (attestation, the sealed channel, passkeys and API keys), a
client generated from the node's OpenAPI document, and a hand-written layer on
top. It is all synchronous Python (CPython 3.9+, built with maturin). Read
`sdks/AGENTS.md` first for the rules every SDK follows: one version, one
release, scope, generated vs written. Commits and PR titles follow core's
`RELEASING.md`. This SDK releases only with core.

## Names

| What | Name |
| --- | --- |
| PyPI distribution (`pip install`) | `mero-py` (it was `calimero-client-py` up to 1.0.0-rc.30) |
| Python imports | `calimero`, `calimero_client_py`: **unchanged** |
| Rust crate (`Cargo.toml` `[package]`) | `mero-py` |
| Extension module | `calimero_client_py._native` (`[lib] name = "calimero_client_py"`, maturin's `module-name`) |

Only the distribution and the crate were renamed. Never rename the import
names, `[lib] name` or `module-name`: every user's `import` would break.
`calimero.__version__` reads the installed `mero-py` distribution's version.

## Layout

```
src/                         # the Rust extension (PyO3): calimero_client_py._native
calimero_client_py/          # the Python package
  client.py, sealed.py, _facade.py, recovery.py   # hand-written
  openapi/                   # GENERATED: openapi-python-client's client of the node API
  facade.py                  # GENERATED: one Client method per operation
calimero/                    # the `calimero` package: re-exports calimero_client_py
openapi/generator.yaml       # openapi-python-client's settings
scripts/                     # generate.sh (sdk.json `generate`), the generators, build.sh
tests/                       # pytest: unit tests, plus cluster tests against a merod
examples/                    # example_usage.py (compiled by CI)
abi-codegen/                 # calimero-abi-codegen: Python clients from an app's ABI
docs/                        # the docs site's source (Astro; reference/methods.mdx is GENERATED)
```

## Build, test, lint

Run these from this directory. cargo uses core's pinned toolchain from the
root `rust-toolchain.toml`. The steps are the ones `sdk-mero-py.yml` runs:

```bash
python3 -m venv .venv && . .venv/bin/activate
pip install maturin==1.15.0 && pip install -e ".[dev]"   # builds the extension
black --check .
pip install openapi-python-client==0.26.2
python scripts/generate_openapi_client.py --check         # generated code is current
cargo fmt --check
cargo test                   # without --features python, on purpose (see Cargo.toml)
pytest tests/ -v --tb=short  # the cluster tests skip without merod
python -m py_compile examples/*.py
(cd abi-codegen && pip install -e . pytest pytest-cov && pytest)
```

`[profile.release]` must not set `panic = "abort"`: pyo3 turns a panic into a
Python exception only when it unwinds.

`Cargo.toml` has its own `[workspace]`, and this directory keeps its own
`Cargo.lock`. The crate is not a member of core's workspace (the root
`Cargo.toml` lists its members explicitly) and must not become one.

### Core's crates: this tree's, by path

`calimero-client`, `calimero-identity` and `calimero-sealed` are path
dependencies on this tree's `crates/`. They pull in `calimero-tee-attestation`
and `calimero-tee-release` too. The extension therefore always builds against
the core it ships with: a change to those crates that breaks the client fails
this SDK's CI in the same pull request. `sdk.json` `inputs` lists those five
crates, plus core's root `Cargo.toml` (their workspace settings) and
`rust-toolchain.toml`. Update that list when the client starts using another
core crate. `cargo metadata` in this directory lists every path package.

The sdist still works. `maturin sdist` copies the five crates, and core's root
`Cargo.toml` for their `workspace = true` settings, into the archive under
their own paths, and a wheel builds from the unpacked sdist alone (CI's
`mero-py sdist builds on its own` job).

## Generated code

The generated code comes from core's OpenAPI document, read straight from this
tree (`../../../docs/public/openapi.json`). Nothing is vendored or fetched.

- `calimero_client_py/openapi/`: openapi-python-client 0.26.2's output.
- `calimero_client_py/facade.py`: `scripts/generate_facade.py`, one method per operation.
- `docs/src/content/docs/reference/methods.mdx`: `scripts/generate_api_docs.py`.
- `abi-codegen/examples/generated_client/`: `calimero-abi-codegen` on its example schema.

```bash
python3 scripts/sdks.py generate mero-py    # from core's root; or bash scripts/generate.sh here
```

`scripts/generate.sh` runs the generators and Black at the pinned versions in a
venv under `target/`. Never edit generated files. To change them, fix the
server (the document), the generator scripts or `openapi/generator.yaml`, then
regenerate. `scripts/facade_changes.py OLD NEW` lists the facade methods a
regeneration added, removed or changed.

## Cluster tests

`tests/test_cloud_v2*.py`, `test_facade_cluster.py`, `test_account_recovery.py`,
`test_org_recovery.py` and `test_upgrade_keys.py` start a cluster of `merod`
and run the client against it. They read `MEROD_BINARY` and `KV_STORE_WASM`
(both default to this core tree's `target/`) and skip when the binary is
missing. `MEROD_PORT_BASE=3100` keeps the nodes' ports counting up from 3100
instead of the OS's picks, for a machine that runs other clusters. From core's root, built as e2e.yml builds them:

```bash
cargo build --release -p merod -p meroctl
target/release/meroctl app build apps/v2-fixtures/kv --out target/app-components/kv.wasm
cd sdks/clients/mero-py && . .venv/bin/activate && pip install -e ".[dev]"
MEROD_BINARY=$PWD/../../../target/release/merod \
KV_STORE_WASM=$PWD/../../../target/app-components/kv.wasm \
  pytest tests/test_cloud_v2.py tests/test_cloud_v2_api.py -v --tb=short
```

Core's `e2e.yml` (`clients` job) runs these against the release `merod` it
builds. Core's `crates/app-abi/tests/py` (`smoke.py` and mypy on the golden
clients) imports `calimero_client_py.facade` from this package.

## Version

The version is core's root `VERSION`. `Cargo.toml` states it (`1.0.0-rc.30`).
`pyproject.toml` takes it from there (`dynamic = ["version"]`), and maturin
writes the PEP 440 form (`1.0.0rc30`) into the wheel. `Cargo.lock` and
abi-codegen also state it. All four are `sdk.json` `version_files`; change
them only with `python3 scripts/sdks.py version-sync`.

## CI and release

`.github/workflows/sdk-mero-py.yml` is called by `sdks.yml` (tests) and, on a
core tag, by `release.yml` with `publish: true`. The publish path:

1. builds the sdist and one abi3 wheel per platform (Linux x86_64 and aarch64
   manylinux2014, macOS arm64 and x86_64, Windows x86_64);
2. refuses to publish unless every file is `mero_py` at VERSION and no
   platform is missing;
3. uploads to PyPI by trusted publishing (environment `pypi`). With
   `dry-run: true` it stops before the upload.

The caller's job must grant `id-token: write`. PyPI's trusted publisher for
`mero-py` names core's `release.yml` and the `pypi` environment.

`CHANGELOG.md` holds the history from the calimero-client-py repository. New
entries go in core's release notes.
