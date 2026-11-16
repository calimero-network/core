# mero-react - agent guide

> **Before committing, titling a PR, or releasing, read [RELEASING.md](RELEASING.md).**
> PRs are squash-merged and the PR title is the release commit: its type decides
> whether this repository releases, and which version (the table in RELEASING.md).

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
- `docs/`: the static docs site `static.yml` deploys to Pages.
- `example/`: a separate demo project (its own `package.json` and lockfile), not part of the package.

## Build, test, lint

pnpm (version pinned by `packageManager` in `package.json`), Node 24 in CI.
These are the steps `ci.yml` runs:

```bash
pnpm install --frozen-lockfile
pnpm build                # tsup; CI checks every file package.json `exports` names exists
pnpm test                 # vitest
pnpm typecheck
pnpm lint                 # eslint src
```

`pnpm storybook` runs the component stories locally.

## Generated code: core's OpenAPI document, through mero-js

This repository has no generated code of its own. The node's routes, their
request and response types, and the method groups the hooks call (`client.apps`,
`client.orgs`, `client.auth`, ...) are generated in mero-js (`src/generated/`)
from core's OpenAPI document. Do not hand-write a route, body or response type
here, or copy generated code in: a missing or wrong route is fixed in core's
document, picked up by mero-js's regeneration and release, and then reaches this
package through the `@calimero-network/mero-js` dependency.
