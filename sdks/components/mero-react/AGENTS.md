# mero-react - agent guide

> mero-react lives in calimero-network/core under `sdks/components/mero-react`
> and ships with core: one version (core's root `VERSION`), one tag, one release,
> published right after mero-js. Read [sdks/AGENTS.md](../../AGENTS.md) (the SDK
> rules) and core's [RELEASING.md](../../../RELEASING.md) before committing or
> titling a PR. Never edit `version` in `package.json` by hand
> (`python3 scripts/sdks.py version-sync`).

`@calimero-network/mero-react` is the React binding for the Calimero 1.0 client:
one mero-js `CalimeroClient` in context, and hooks and components on top of it
(passkey sign-in, sessions, app calls and events, organizations, invites, consent,
blobs, presence). It re-exports every mero-js export. See [README.md](README.md).

## Layout

- `src/context/`: `MeroContext`, the provider that holds the client.
- `src/hooks/`: the hooks, one file per area.
- `src/components/`: `ConnectButton`, `LoginModal` and their Storybook stories.
- `src/node-shield/`: `NodeShield`, the separate `@calimero-network/mero-react/node-shield` entry.
- `src/__tests__/`: the vitest tests (`fake.tsx` is their fake client); `src/test-utils/` holds `MockMeroProvider`.
- `examples/`: one directory per example (see below).
- `docs/`: the static docs site (plain HTML).
- `sdk.json`: how core's CI treats this SDK (`scripts/sdks.py`).

## mero-js: the workspace's own

`@calimero-network/mero-js` is `workspace:*`: the pnpm workspace in `sdks/`
links `sdks/clients/mero-js`, and publishing writes its exact version (the same
`VERSION`). mero-react resolves it through mero-js's `exports`, i.e. its built
`dist/`, so **build mero-js first**; a change to mero-js runs this package's CI
too (`depends_on` in `sdk.json`).

## Build, test, lint

pnpm and its version come from `sdks/package.json`; the one lockfile is
`sdks/pnpm-lock.yaml`. Node 24 in CI. These are the steps
`.github/workflows/sdk-mero-react.yml` runs:

```bash
cd sdks && pnpm install --frozen-lockfile
pnpm --filter @calimero-network/mero-js build
cd components/mero-react
pnpm build                # tsup; CI checks every file package.json `exports` names exists
pnpm build:examples       # typechecks and builds examples/kv-store
pnpm test                 # vitest
pnpm typecheck
pnpm lint                 # eslint src
```

`pnpm storybook` runs the component stories locally.

## Generated code: core's OpenAPI document, through mero-js

This package has no generated code of its own (`sdk.json` has no `generate`).
The node's routes, their request and response types, and the method groups the
hooks call (`client.apps`, `client.orgs`, `client.auth`, ...) are generated in
mero-js (`src/generated/`) from core's `docs/public/openapi.json`. Do not
hand-write a route, body or response type here, or copy generated code in: a
missing or wrong route is fixed in the server or mero-js's generator, and
reaches this package through the workspace.

## Examples

`examples/kv-store/`: a browser app for core's `apps/kv-store` (passkey sign-in,
`useApps`, `useView`, `useExecute`, `useSubscription`). It has no package.json
of its own: it uses this package's dev dependencies (Vite, React), and Vite
resolves `@calimero-network/mero-react` to `src/`. `pnpm build:examples`
typechecks it (`tsc -p examples/kv-store/tsconfig.json`) and builds it into
`examples/kv-store/dist`; `pnpm dev:example` serves it. A new example gets its
own directory and is added to `build:examples`.
