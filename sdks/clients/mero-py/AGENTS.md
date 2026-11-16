# calimero-client-py - agent guide

> **Before committing, titling a PR, or releasing, read [RELEASING.md](RELEASING.md).**
> PRs are squash-merged and the PR title is the release commit: its type decides
> whether this repository releases, and which version (the table in RELEASING.md).

`calimero-client-py` is the Python client for Calimero 1.0: a PyO3 extension over
core's Rust `calimero-client` (attestation, the sealed channel, passkeys and API
keys) plus a generated client for the node's API, all synchronous Python
(CPython 3.9+, built with maturin). See [README.md](README.md) and the docs site,
whose source is `docs/`.

## Layout

- `src/`: the Rust extension (PyO3); `Cargo.toml` pins core's crates by tag.
- `calimero_client_py/`: the Python package; `client.py`, `sealed.py` and `_facade.py` are hand-written.
- `calimero_client_py/openapi/`, `calimero_client_py/facade.py`: **generated, do not edit** (see below).
- `tests/`: pytest; the cluster tests skip unless `MEROD_BINARY` (or a sibling `core` checkout) and `KV_STORE_WASM` are found.
- `abi-codegen/`: a separate Python package (its own `pyproject.toml` and tests) that generates wrappers from an app's ABI.
- `scripts/`: the generators, `refresh_openapi.py`, and the workflow checks CI runs.

## Build, test, lint

These are the steps `ci.yml` runs (Python 3.11, Rust stable):

```bash
pip install maturin==1.15.0
pip install -e ".[dev]"             # builds the extension with maturin
black --check .
pip install openapi-python-client==0.26.2
python scripts/generate_openapi_client.py --check
cargo fmt --check
cargo test                          # without --features python, on purpose
pytest tests/ -v --tb=short
```

- `abi-codegen/`: `cd abi-codegen && pip install -e . pytest && pytest`.
- Workflow checks: `python scripts/check_oidc_jobs.py` and
  `python scripts/check_workflow_hardening.py` (each with its `scripts/test_*.py`).
- `[profile.release]` in `Cargo.toml` must not set `panic = "abort"`; CI fails if it does.
- The docs site (`docs/`, npm) is checked with `npm ci && npm run check` in `docs/`.

## Generated code: core's OpenAPI document

`calimero_client_py/openapi/` (openapi-python-client's output) and
`calimero_client_py/facade.py` (one method per operation, by
`scripts/generate_facade.py`) are generated from core's OpenAPI document, vendored
at `openapi/openapi.json` (the core commit is in `openapi/source.json`), and
checked in. Never hand-edit them; CI's `--check` fails on any drift. The same run
also rewrites `docs/src/content/docs/reference/methods.mdx`.

- Regenerate: `python scripts/generate_openapi_client.py`
  (needs `openapi-python-client==0.26.2` and `black==26.10.1`).
- Take a newer document from core: `python scripts/refresh_openapi.py [--ref <core ref>]`, then regenerate.
- A wrong route or type is fixed in core's document, the generator scripts or
  `openapi/generator.yaml`, not in the output. `openapi-sync.yml` opens the sync PRs from core on its own.
