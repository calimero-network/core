# node-script: mero-js example

A Node script (`index.ts`) that checks a node is up (`client.public.health()`),
then, with an API key, writes and reads core's `apps/kv-store`
(`client.apps.call` / `client.apps.view`). It compiles against the built
package, as an outside consumer would. From `sdks/clients/mero-js`:

```bash
pnpm build                 # the package's dist/, which the example imports
pnpm build:examples        # tsc -p examples/node-script → examples/node-script/dist
NODE_URL=http://127.0.0.1:2501 ALLOW_MOCK=1 API_KEY=<key> APP=<app id> \
  node examples/node-script/dist/index.js
```

`ALLOW_MOCK=1` accepts a dev node's mock attestation (`merod run --dev`);
otherwise the node is attested under `MIN_RELEASE` (default `2.3.109`).
