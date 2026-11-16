# mero-js - agent guide

> mero-js lives in calimero-network/core under `sdks/clients/mero-js` and ships
> with core: one version (core's root `VERSION`), one tag, one release. Read
> [sdks/AGENTS.md](../../AGENTS.md) (the SDK rules) and core's
> [RELEASING.md](../../../RELEASING.md) before committing or titling a PR. Never
> edit `version` in `package.json` by hand (`python3 scripts/sdks.py version-sync`).

`@calimero-network/mero-js` is the JavaScript/TypeScript client for Calimero 1.0
(browser and Node 18+): `CalimeroClient` verifies a node's TDX attestation, seals
every request to it, signs people in with passkeys and machines with API keys, and
calls apps. See [README.md](README.md) and the docs site, whose source is `docs/`.

## Layout

- `src/runtime/`: attestation, directory, passkey and transport.
- `src/sealed/`: the sealed channel (Noise) and release checks.
- `src/package/`: building and signing a compute-only `.mpk` (golden vectors in `fixtures/` come from core's `calimero-bundle`).
- `src/routes/`: the route client the generated methods call through.
- `src/client/`: `CalimeroClient` and its helpers. `receipts.ts` mirrors core's `calimero-client` receipt checks; its vectors, `fixtures/receipt-vectors.json`, are a copy of core's `crates/client/tests/fixtures/receipt-vectors.json` made by `pnpm generate`, never hand-edited.
- `src/generated/`: **generated, do not edit** (see below).
- `tests/cluster/`: the cluster suite (`vitest.cluster.config.ts`); `tests/consumer/` is what `typecheck:consumer` compiles; unit tests sit beside the code as `src/**/*.test.ts`, and `typecheck:tests` (`tsconfig.test.json`) compiles them with `tests/cluster` and `tests/support`.
- `examples/`: one directory per example (see below).
- `docs/`: the docs site (Astro/Starlight, its own npm project, not in the workspace).
- `sdk.json`: how core's CI treats this SDK (`scripts/sdks.py`).

## Build, test, lint

The package is a member of the pnpm workspace in `sdks/` (pnpm and its version
come from `sdks/package.json`; the one lockfile is `sdks/pnpm-lock.yaml`). Node 20
in CI. These are the steps `.github/workflows/sdk-mero-js.yml` runs:

```bash
cd sdks && pnpm install --frozen-lockfile && cd clients/mero-js
pnpm check:api            # regenerates (pnpm generate) and fails on any diff
pnpm build
pnpm build:examples
pnpm test:unit
pnpm typecheck
pnpm typecheck:consumer   # tests/consumer against the built dist/, strict NodeNext
pnpm typecheck:tests      # tsconfig.json excludes *.test.ts; this checks them
pnpm lint                 # eslint; pnpm lint:fix to fix
```

## Generated code: core's OpenAPI document

`src/generated/api.ts` and `src/generated/methods.ts` are generated from the node
API document in this same tree, `docs/public/openapi.json` (read as
`../../../docs/public/openapi.json`, no network, no vendored copy). Never
hand-edit them.

- `pnpm generate` (what `sdk.json`'s `generate` runs, and
  `python3 scripts/sdks.py generate mero-js` from core's root) regenerates
  everything `sdk.json` lists under `generated`: `pnpm gen:api`
  (`openapi-typescript`, then `scripts/gen-methods.mjs`) and
  `pnpm refresh:receipt-vectors` (copies core's receipt vectors).
- `pnpm check:api` regenerates and fails when the committed output differs.
- A wrong route, body or type is fixed in the server (which regenerates
  `docs/public/openapi.json`) or in the generator, not in the output. When the
  document changes on master, core's `sdk-regenerate.yml` opens the pull request
  that regenerates every SDK.

## Examples

`examples/node-script/`: a Node script that checks a node's health and calls
core's `apps/kv-store` with an API key. `pnpm build:examples` compiles it with
`tsc` against the built `dist/` (as an outside consumer would, through the
package's own `exports`), so run `pnpm build` first. A new example gets its own
directory and is added to `build:examples`.

## Cluster tests against this tree's merod

`pnpm test:cluster` runs `tests/cluster/` against a real merod: the suite starts
three `merod run --dev` nodes itself (`tests/cluster/cluster.ts`) from
`MEROD_BINARY` and installs the kv component from `KV_STORE_WASM`. Both default
to this tree's build (`target/debug/merod`, `target/app-components/kv.wasm`),
so from core's root:

```bash
cargo build -p merod -p meroctl                             # target/debug/merod, meroctl
cargo run -p calimero-app-build --example build-test-apps   # target/app-components/*.wasm (kv, compute, ...)
cd sdks/clients/mero-js && pnpm build && pnpm test:cluster
```

Against a released merod instead, build the fixtures with its meroctl, as
`e2e.yml` does, and point the suite at them:

```bash
meroctl app build apps/v2-fixtures/kv --out target/app-components/kv.wasm
meroctl app build apps/v2-fixtures/compute --out target/app-components/compute-app.wasm \
  --compute-out target/app-components/compute.wasm
meroctl app build apps/rooms --out target/app-components/rooms.wasm
cd sdks/clients/mero-js
MEROD_BINARY=<merod> MEROCTL=<meroctl> \
COMPUTE_WASM=$PWD/../../../target/app-components/compute.wasm \
COMPUTE_FIXTURE_DIR=$PWD/../../../apps/v2-fixtures/compute MERO_JS_REQUIRE_COMPUTE=1 \
ROOMS_WASM=$PWD/../../../target/app-components/rooms.wasm \
  pnpm test:cluster
```

The nodes take ports the OS picks; `MERO_JS_CLUSTER_PORT_BASE=2900` takes the
first free ones from 2900 up instead (to keep clear of other local clusters).

Optional: `MEROCTL=<meroctl>` starts a job node, and with
`COMPUTE_WASM=target/app-components/compute.wasm`
`COMPUTE_FIXTURE_DIR=apps/v2-fixtures/compute` the pipeline test runs
(`MERO_JS_REQUIRE_COMPUTE=1` makes it fail instead of skip); `ROOMS_WASM` runs
the rooms test; `MEROD_V2_LOGS=1` shows the nodes' logs. Core's `e2e.yml`
(`clients`) sets these against its release build.
`scripts/download-released-merod.sh` fetches a released merod and kv fixture
instead, as the old repository's CI did.
