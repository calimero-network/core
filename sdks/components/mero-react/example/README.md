# KV Store - Demo application

This repository contains two independent parts:

- `logic/` — Rust smart-contract (compiled to WASM)
- `app/` — React frontend (Vite) that talks to the contract via generated ABI client

The folders are separate projects; treat each as its own repo.

## Calimero 1.0

The frontend (`app/src/App.tsx`) uses mero-react 11 on the 1.0 client: passkey sign-in, `useApps`, `useView('entries')`, `useExecute().execute('set', …)` and `useSubscription` for live updates.

- **Bundled and served by the node** (the default): the page runs at `{label}.{apps-domain}` with `servedByNode`. Set `VITE_MIN_RELEASE` to the oldest mero-tee release to trust.
- **Against `merod local`** (Linux, or Docker on macOS): `cargo run -p merod -- local`, install the bundle and create an instance with `meroctl`, then run `VITE_NODE_URL=http://127.0.0.1:2501 pnpm --dir app dev`. That turns on `allowMock` (mock attestation, dev only).
- **Against a Calimero Cloud cluster**: `VITE_CLUSTER=eu-shared-1 pnpm --dir app dev`. The client reads the cluster's nodes from the node directory, attests each under `VITE_MIN_RELEASE`, and fails over between them.

The `network:bootstrap` script below still drives merobox, which 1.0 retired; use `merod local` instead.

## Prerequisites

- pnpm (or npm) for JavaScript tooling
- Rust toolchain + wasm target: `rustup target add wasm32-unknown-unknown`
- Optional: `wasm-opt` for size optimization

## Logic (Rust)

```bash
pnpm run logic:build
```

Optional cleanup:

```bash
pnpm run logic:clean
```

### Contract capabilities

- Key-value operations: `set`, `get`, `remove`, `entries`, `len`, `clear`
- Utility: `get_result`, `get_unchecked`
- Demo helper: `create_random_entry()` — inserts `key-<n>` with `value-<n>` and returns the key

Events emitted: `Inserted`, `Updated`, `Removed`, `Cleared`

### Build artifacts

- Built WASM outputs to `logic/res/<crate_name>.wasm` (minified if `wasm-opt` is available)
- ABI JSON is expected at `logic/res/abi.json`

## App (React)

```bash
cd app && pnpm install
```

Build and run:

```bash
pnpm --dir app build
pnpm --dir app dev
```

Open the app in your browser and connect to a running node.

Docs: https://calimero-network.github.io/build/quickstart

## Watchers and Dev Workflow

The root `app:dev` script runs the web app alongside a unified watcher for `logic/res/`.

```bash
pnpm run app:dev
```

What happens:

- `logic:watch`: watches `logic/res/**/*`
  - On `abi.json` change → runs codegen: `app:generate-client`
  - On `*.wasm` change → copies the changed file to data nodes via `logic:sync`

Key scripts (root `package.json`):

- `logic:watch`: `chokidar "logic/res/**/*" -c "node scripts/on-res-change.mjs {path}"`
- `logic:sync`: `bash ./scripts/sync-wasm.sh <path>` — copies to `data/calimero-node-1/` and `data/calimero-node-2/`
- `app:generate-client`: `npx @calimero-network/abi-codegen@0.1.1 -i logic/res/abi.json -o app/src/api`
- `app:dev`: `concurrently` runs the Vite dev server and `logic:watch`

Notes:

- The watcher only triggers when `logic/res/` changes. Make sure your build writes there.
- `sync-wasm.sh` copies by filename (basename) so any wasm produced in `res/` is propagated.

## ABI Codegen

Client types and a thin client are generated into `app/src/api` from `logic/res/abi.json`.

- This is runs automatically by the watcher on `abi.json` changes.

## Merobox (Local Network)

You can bootstrap a local network with Merobox:

```bash
pnpm run network:bootstrap
```

This runs the workflow defined in `workflows/workflow-example.yml` and starts local Calimero nodes whose data dirs live under `data/`.

## Typical Dev Loop

1) Start dev (web + watchers):

```bash
pnpm run app:dev
```

2) Edit Rust contract under `logic/src` and build:

```bash
pnpm run logic:build
```

When the wasm in `logic/res/` updates, the watcher copies it to `data/calimero-node-1/` and `data/calimero-node-2/` automatically.

## Troubleshooting

- If `concurrently` or `chokidar` are missing, install dev deps at repo root:

```bash
pnpm add -D concurrently chokidar-cli
```

- If ABI codegen fails due to missing schema, ensure you’re on `@calimero-network/abi-codegen@0.1.1` (the script pins this version).

