# kv-store: mero-react example

A browser app for core's `apps/kv-store` (a string key-value store): passkey
sign-in with `ConnectButton`, `useApps` to pick an instance, `useView('entries')`,
`useExecute().execute('set' | 'remove', …)` and `useSubscription` for live
updates. All of it is `src/App.tsx`.

It builds from this tree: Vite resolves `@calimero-network/mero-react` to
`../../src/index.ts` and `@calimero-network/mero-js` to the workspace's mero-js.
From `sdks/`:

```bash
pnpm install
pnpm --filter @calimero-network/mero-js build
pnpm --filter @calimero-network/mero-react build:examples   # typecheck + vite build into examples/kv-store/dist
```

Run it (`pnpm --filter @calimero-network/mero-react dev:example`) against:

- **a dev node**: `VITE_NODE_URL=http://127.0.0.1:2501` (turns on `allowMock`,
  mock attestation, dev only), with `apps/kv-store` installed and an instance
  created with `meroctl`;
- **a Calimero Cloud cluster**: `VITE_CLUSTER=eu-shared-1`;
- **the node that serves it** (the default, when bundled with the app):
  `servedByNode`. `VITE_MIN_RELEASE` sets the oldest mero-tee release to trust
  (default `2.3.109`).
