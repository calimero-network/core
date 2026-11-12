// What a caller of the generated clients writes, type-checked against the published mero-js
// (`npm run check`) and run against a test double (`node clients.ts`): a real
// `CalimeroClient` is the transport, a double needs only `AppTransport` (`AppCallTransport`
// without events), the options reach the right request, `raw` answers the log index and
// events, and `after` runs after every request, refused ones included.

import assert from 'node:assert/strict';

import { CalimeroClient, passkeyConfirm } from '@calimero-network/mero-js';
import type { NodeEvent, PasskeyProvider } from '@calimero-network/mero-js';

import { KitchenSinkClient } from '../golden/kitchen-sink.ts';
import type { Adjacent, Either, InitArgs, Item, Page, Status, Tagged, Tree } from '../golden/kitchen-sink.ts';
import { KvStoreClient } from '../golden/kv-store.ts';
import type {
  AppCallTransport,
  AppTransport,
  AppEvent,
  CallAnswer,
  KvStoreApi,
  KvStoreRawApi,
  ViewAnswer,
} from '../golden/kv-store.ts';
import { RoomsClient } from '../golden/rooms.ts';

// Type-level only: never called, so nothing here touches a node.
export function withARealClient(client: CalimeroClient, passkey: PasskeyProvider): Promise<unknown>[] {
  const kv = new KvStoreClient(client, 'app-1', { after: () => void client.session });
  const rooms = new RoomsClient(client, 'app-1');
  const sink = new KitchenSinkClient(client, 'app-1');
  return [
    kv.set({ key: 'a', value: 'b' }, { idempotencyKey: 'k-1' }),
    kv.get({ key: 'a' }, { consistency: 'stale', minIndex: 3 }),
    rooms.adminRelease({ id: 'b-1', reason: 'late' }, { confirm: passkeyConfirm(passkey) }),
    // An `option` argument may be left out or be `null`.
    sink.wipe({}),
    sink.wipe({ reason: null }),
  ];
}

// A transport without the event stream: `Pick<CalimeroClient, 'apps'>` calls and views, and a
// client over it has no `events()`. A client over the whole `CalimeroClient` is the default
// `KvStoreClient`, as callers declare it.
export function withCallsAlone(client: CalimeroClient): unknown[] {
  const apps: Pick<CalimeroClient, 'apps'> = client;
  const kv = new KvStoreClient(apps, 'app-1');
  const whole: KvStoreClient = new KvStoreClient(client, 'app-1');
  const api: KvStoreApi = whole;
  // @ts-expect-error a client over calls and views alone is not one with events
  const narrowed: KvStoreClient = kv;
  return [
    kv.get({ key: 'a' }),
    kv.raw.set({ key: 'a', value: 'b' }),
    // @ts-expect-error `events()` needs a transport with `events`
    kv.events(),
    whole.events(),
    api,
    narrowed,
  ];
}

// `raw` answers a method whole: a call's log index and events, a view's index.
// A test hands code that reads them a double of the `raw` methods it calls.
export async function resumeAfter(kv: Pick<KvStoreRawApi, 'set' | 'get'>): Promise<number> {
  const set: CallAnswer<null> = await kv.set({ key: 'a', value: 'b' });
  const emitted: AppEvent[] = set.events;
  const read: ViewAnswer<string | null> = await kv.get({ key: 'a' }, { minIndex: set.index });
  return emitted.length + read.index;
}
export function withRaw(client: CalimeroClient): Promise<number>[] {
  return [
    resumeAfter(new KvStoreClient(client, 'app-1').raw),
    resumeAfter({
      set: async () => ({ result: null, index: 3, events: [] }),
      get: async () => ({ result: 'b', index: 3 }),
    }),
  ];
}

// An app whose `init` takes arguments declares them as `InitArgs`, what `apps.create` sends.
export function createSink(client: CalimeroClient): Promise<unknown>[] {
  const initArgs = { owner: 'alice', limit: '10', first: null } satisfies InitArgs;
  return [
    client.apps.create({ package: 'com.example.kitchen-sink', initArgs }),
    // @ts-expect-error a u64 init argument is a decimal string
    client.apps.create({ initArgs: { owner: 'alice', limit: 10 } satisfies InitArgs }),
  ];
}

// Code that takes the methods alone (`KvStoreApi`) is handed the class, or in a test a double
// of the methods it calls, with no transport underneath.
export function readKey(kv: Pick<KvStoreApi, 'get'>): Promise<string | null> {
  return kv.get({ key: 'a' });
}
export function withTheApi(client: CalimeroClient): Promise<string | null>[] {
  const api: KvStoreApi = new KvStoreClient(client, 'app-1');
  return [readKey(api), readKey({ get: async ({ key }) => key })];
}

// The app types are what serde writes: records with D20 fields, each enum representation,
// generics instantiated where used.
export const shapes = {
  status: ['Open', { Closed: { at: '1' } }, { Moved: 'x' }, { Span: [1, 2] }] satisfies Status[],
  tagged: [{ type: 'draft' }, { type: 'published', at: '3' }] satisfies Tagged[],
  adjacent: [{ t: 'Text', c: 'hi' }, { t: 'Empty' }] satisfies Adjacent[],
  either: [7, { name: 'n' }] satisfies Either[],
  page: { items: [{ id: '1', title: 't', tags: [], status: 'Open' }], next: null } satisfies Page<Item>,
  tree: { value: 1, children: [{ value: 2, children: [] }] } satisfies Tree,
  // @ts-expect-error a u64 field is a decimal string
  number: { id: 1, title: 't', tags: [], status: 'Open' } satisfies Item,
  // @ts-expect-error an externally tagged variant with data is an object
  bare: 'Closed' satisfies Status,
};

type Sent = { kind: 'call' | 'view'; args: Record<string, unknown>; options: Record<string, unknown> | undefined };

function double(results: unknown[]): { transport: AppTransport; sent: Sent[] } {
  const sent: Sent[] = [];
  const answer = async () => {
    const result = results.shift();
    if (result instanceof Error) throw result;
    return { result };
  };
  const transport: AppTransport = {
    apps: {
      call: (args, options) => (sent.push({ kind: 'call', args, options }), answer()),
      view: (args, options) => (sent.push({ kind: 'view', args, options }), answer()),
    },
    async *events(): AsyncGenerator<NodeEvent> {
      yield { id: '7', kind: 'Inserted', data: { key: 'a', value: 'b' } };
    },
  };
  return { transport, sent };
}

const { transport, sent } = double(['b', null, new Error('refused'), '2']);
let after = 0;
const kv = new KvStoreClient(transport, 'app-1', { after: () => void after++ });

assert.equal(await kv.get({ key: 'a' }), 'b');
assert.equal(await kv.set({ key: 'a', value: 'b' }, { idempotencyKey: 'k-1' }), null);
await assert.rejects(kv.remove({ key: 'a' }), /refused/);
assert.equal(await kv.len({ consistency: 'stale', minIndex: 4 }), '2');
assert.equal(after, 4, 'after runs once per request, refused ones included');
assert.deepEqual(sent, [
  { kind: 'view', args: { app: 'app-1', method: 'get', args: { key: 'a' } }, options: {} },
  { kind: 'call', args: { app: 'app-1', method: 'set', args: { key: 'a', value: 'b' }, idempotencyKey: 'k-1' }, options: {} },
  { kind: 'call', args: { app: 'app-1', method: 'remove', args: { key: 'a' } }, options: {} },
  { kind: 'view', args: { app: 'app-1', method: 'len', args: {}, consistency: 'stale', minIndex: 4 }, options: {} },
]);

const events: AppEvent[] = [];
for await (const event of kv.events()) events.push(event);
assert.deepEqual(events, [{ id: '7', kind: 'Inserted', data: { key: 'a', value: 'b' } }]);

// `raw` answers the same requests whole; a double that leaves out `index` and `events` reads as
// index 0 and none.
const answers = [
  { result: null, index: 9, events: [{ kind: 'Inserted', data: { key: 'a', value: 'b' } }] },
  { result: 'b', index: 9 },
  { result: null },
];
const indexed: AppCallTransport = {
  apps: {
    call: async () => answers.shift() ?? { result: null },
    view: async () => answers.shift() ?? { result: null },
  },
};
const raw = new KvStoreClient(indexed, 'app-1').raw;
assert.deepEqual(await raw.set({ key: 'a', value: 'b' }), {
  result: null,
  index: 9,
  events: [{ kind: 'Inserted', data: { key: 'a', value: 'b' } }],
});
assert.deepEqual(await raw.get({ key: 'a' }, { minIndex: 9 }), { result: 'b', index: 9 });
assert.deepEqual(await raw.clear(), { result: null, index: 0, events: [] });

// A `confirm` method's step-up answer goes to the request's options, not its body.
const confirm = double([null]);
const confirmCallback = async () => {
  throw new Error('not asked');
};
await new RoomsClient(confirm.transport, 'app-1').adminRelease({ id: 'b-1', reason: 'late' }, { confirm: confirmCallback });
assert.deepEqual(confirm.sent, [
  {
    kind: 'call',
    args: { app: 'app-1', method: 'adminRelease', args: { id: 'b-1', reason: 'late' } },
    options: { confirm: confirmCallback },
  },
]);

console.log('generated clients: ok');
