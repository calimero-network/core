# mero-js - agent guide

> **Before committing, titling a PR, or releasing, read [RELEASING.md](RELEASING.md).**
> PRs are squash-merged and the PR title is the release commit: its type decides
> whether this repository releases, and which version (the table in RELEASING.md).

`@calimero-network/mero-js` is the JavaScript/TypeScript client for Calimero 1.0
(browser and Node 18+): `CalimeroClient` verifies a node's TDX attestation, seals
every request to it, signs people in with passkeys and machines with API keys, and
calls apps. See [README.md](README.md) and the docs site, whose source is `docs/`.

## Layout

- `src/runtime/`: attestation, directory, passkey and transport.
- `src/sealed/`: the sealed channel (Noise) and release checks.
- `src/package/`: building and signing a compute-only `.mpk` (golden vectors in `fixtures/` come from core's `calimero-bundle`).
- `src/routes/`: the route client the generated methods call through.
- `src/client/`: `CalimeroClient` and its helpers. `receipts.ts` mirrors core's `calimero-client` receipt checks (the receipt and each job's report); its vectors in `fixtures/receipt-vectors.json` are core's: `pnpm refresh:receipt-vectors`, never hand-edited.
- `src/generated/`: **generated, do not edit** (see below).
- `tests/cluster/`: the cluster suite (`vitest.cluster.config.ts`); `tests/consumer/` is what `typecheck:consumer` compiles; unit tests sit beside the code as `src/**/*.test.ts`, and `typecheck:tests` (`tsconfig.test.json`) compiles them with `tests/cluster` and `tests/support`.

## Build, test, lint

pnpm (version pinned by `packageManager` in `package.json`), Node 20 in CI.
These are the steps `ci.yml` runs:

```bash
pnpm install --frozen-lockfile
pnpm check:api            # regenerates src/generated and fails on any diff
pnpm build
pnpm test:unit
pnpm typecheck
pnpm typecheck:consumer
pnpm typecheck:tests      # tsconfig.json excludes *.test.ts; this checks them
pnpm lint                 # eslint; pnpm lint:fix to fix
```

`pnpm test:cluster` runs `tests/cluster/` against a real merod 1.0: CI fetches one with
`scripts/download-released-merod.sh` and sets `MEROD_BINARY` and `KV_STORE_WASM`.
The docs site (`docs/`, npm, Node 24) is checked with `npm ci && npm run check` in `docs/`.

## Generated code: core's OpenAPI document

`src/generated/api.ts` and `src/generated/methods.ts` are generated from core's node
API document, vendored at `openapi/core-openapi.json` (the core commit is in
`openapi/SOURCE`). Never hand-edit them; CI's `pnpm check:api` fails on any drift.

- Regenerate from the vendored document: `pnpm gen:api`
  (`openapi-typescript`, then `scripts/gen-methods.mjs`).
- Take a newer document from core: `scripts/refresh-openapi.sh [<core ref>]`
  (`pnpm refresh:openapi`), which also regenerates.
- A wrong route, body or type is fixed in core's document, or in the generator,
  not in the output. `openapi-sync.yml` opens the sync PRs from core on its own.
