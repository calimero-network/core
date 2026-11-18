// A Node script that talks to a Calimero node with an API key: it checks the
// node is up, then reads and writes core's `apps/kv-store` app.
//
//   NODE_URL=http://127.0.0.1:2501 ALLOW_MOCK=1 API_KEY=<key> APP=<app id> \
//     node examples/node-script/dist/index.js
//
// ALLOW_MOCK=1 accepts a dev node's mock attestation (`merod run --dev`); a real
// node is attested under `MIN_RELEASE` (the oldest mero-tee release to trust).
import { CalimeroClient, CalimeroError, type NodePolicy } from '@calimero-network/mero-js';

const env = process.env;
const baseUrl = env.NODE_URL ?? 'http://127.0.0.1:2501';
const policy: NodePolicy = env.ALLOW_MOCK === '1' ? { allowMock: true } : { release: { minReleaseVersion: env.MIN_RELEASE ?? '2.3.109' } };

async function main(): Promise<void> {
  const client = new CalimeroClient({ baseUrl, policy });
  console.log('health:', await client.public.health());

  const { API_KEY: apiKey, APP: app } = env;
  if (!apiKey || !app) {
    console.log('set API_KEY and APP to call an app');
    return;
  }
  client.useApiKey(apiKey);
  await client.apps.call({ app, method: 'set', args: { key: 'greeting', value: 'hello' } });
  const { result } = await client.apps.view({ app, method: 'get', args: { key: 'greeting' } });
  console.log('greeting =', result);
}

main().catch((error: unknown) => {
  console.error(error instanceof CalimeroError ? `${error.code}: ${error.message}` : error);
  process.exitCode = 1;
});
