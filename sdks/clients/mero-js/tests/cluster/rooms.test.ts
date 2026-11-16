/**
 * Rooms (core `apps/rooms`, a `calimero:app` component) end to end through
 * `CalimeroClient` on a local 3-node cluster. Synthetic people and data only.
 * Skipped unless ROOMS_WASM is set:
 *
 *   MEROD_BINARY=.../merod ROOMS_WASM=.../rooms.wasm \
 *   npx vitest run --config vitest.cluster.config.ts tests/cluster/rooms.test.ts
 *
 * Two workarounds until clients are generated from the app's ABI (spec section 6):
 * 64-bit arguments are sent as decimal strings, and methods by their camelCase ABI names.
 */

import { readFileSync } from 'node:fs';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import {
  AppError,
  ArgsTooLarge,
  CalimeroClient,
  ConfirmRequired,
  LoginRequired,
  NotFound,
  type NodeEvent,
} from '../../src/client/index.js';
import { startCluster, type LocalCluster } from './cluster.js';
import { SoftPasskey } from './soft-passkey.js';
import { passkeyConfirm } from '../../src/runtime/passkey.js';

const ORIGIN = 'http://localhost:3000';
const ROOMS_WASM = process.env.ROOMS_WASM as string;
const run = ROOMS_WASM ? describe : describe.skip;
const SLOT_MS = 900_000;

let cluster: LocalCluster;
const client = (i: number) => new CalimeroClient({ baseUrl: cluster.urls[i], policy: { allowMock: true } });

async function person(i: number, name: string): Promise<[CalimeroClient, SoftPasskey, string]> {
  const c = client(i);
  const pk = new SoftPasskey('localhost', ORIGIN);
  await c.signUp(pk, name);
  return [c, pk, (await c.auth.me()).accountId as string];
}

/** Collects `who`'s event stream for `app` from index 0 until `stop()`. */
function listen(who: CalimeroClient, app: string) {
  const seen: NodeEvent[] = [];
  const ac = new AbortController();
  const done = (async () => {
    try {
      for await (const e of who.events(app, 0, ac.signal)) seen.push(e);
    } catch (e) {
      if (!ac.signal.aborted) throw e;
    }
  })();
  const stop = async () => {
    const t = Date.now();
    ac.abort();
    const ended = await Promise.race([done.then(() => true, () => true), new Promise<boolean>((r) => setTimeout(() => r(false), 2000))]);
    console.log('event stream ended after abort:', ended, Date.now() - t, 'ms');
    expect(ended).toBe(true);
  };
  return { seen, stop };
}

async function until(what: string, ok: () => boolean, ms = 10_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!ok()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 100));
  }
}

const kinds = (es: Array<{ kind: string }>) => es.map((e) => e.kind);

beforeAll(async () => {
  if (ROOMS_WASM) cluster = await startCluster(3);
});

afterAll(() => cluster?.stop());

run('rooms on a component cluster, through mero-js', () => {
  it('books, refuses a double booking, hides titles, releases with consent and on no-show, scopes events', async () => {
    // An org: the owner (admin) and two members, each on a different node.
    const [owner, ownerPk] = await person(0, 'owner');
    const [alice, , aliceId] = await person(1, 'alice');
    const [bob, , bobId] = await person(2, 'bob');
    const org = await (await owner.orgs.create({ name: 'Acme' })).orgId;
    for (const p of [alice, bob]) await p.orgs.inviteRedeem({ token: await (await owner.orgs.inviteCreate({ org, role: 'member' }, { confirm: passkeyConfirm(ownerPk) })).token });

    const bundle = await (await owner.packages.bundlePut({ body: new Uint8Array(readFileSync(ROOMS_WASM)), contentType: 'application/wasm' })).hash;
    const app = await (await owner.apps.create({ name: 'rooms', bundle, org })).appId;
    const t0 = Date.now();
    console.log('app', app, 'bundle', bundle);

    const aliceEvents = listen(alice, app);
    const bobEvents = listen(bob, app);

    // Admin-only: a member cannot add a room.
    const notAdmin = await alice.apps.call({ app, method: 'addRoom', args: { id: 'r2', name: 'Side', capacity: 2 } }).catch((e) => e);
    expect(notAdmin).toBeInstanceOf(AppError);
    console.log('member addRoom ->', notAdmin.status, notAdmin.code, notAdmin.appCode, notAdmin.fields);
    const ownerAdd = await owner.apps.call({ app, method: 'addRoom', args: { id: 'r1', name: 'Main', capacity: 8 } });
    console.log('owner addRoom ->', JSON.stringify(ownerAdd));

    // Alice books a future slot; the answer carries only the events she is in the audience of.
    const start = (Math.floor(Date.now() / SLOT_MS) + 4) * SLOT_MS;
    const booked = await alice.apps.call({ app, method: 'book', args: { roomId: 'r1', start: String(start), slotCount: 2, title: 'secret plan' }, idempotencyKey: 'alice-book-1' });
    console.log('book ->', JSON.stringify(booked));
    const id = booked.result as string;
    expect(id).toMatch(/^b/);
    expect(kinds(booked.events).sort()).toEqual(['Booked', 'SlotsTaken']);

    // Same idempotency key: the first answer again, not a second booking.
    const replay = await alice.apps.call({ app, method: 'book', args: { roomId: 'r1', start: String(start), slotCount: 2, title: 'secret plan' }, idempotencyKey: 'alice-book-1' });
    expect(replay.index).toBe(booked.index);
    expect(replay.result).toBe(id);

    // Bob cannot take an overlapping slot.
    const taken = await bob.apps.call({ app, method: 'book', args: { roomId: 'r1', start: String(start + SLOT_MS), slotCount: 1, title: 'x' } }).catch((e) => e);
    expect(taken).toBeInstanceOf(AppError);
    console.log('double book ->', taken.appCode, JSON.stringify(taken.fields));
    expect((taken as AppError).appCode).toMatch(/taken/i);

    // Availability on another node, read-your-writes via minIndex; titles only for holder and admins.
    const day = { roomId: 'r1', dayStart: String(start) };
    const asBob = await bob.apps.view({ app, method: 'availability', args: day, consistency: { stale: true, minIndex: booked.index }.stale ? 'stale' : undefined, minIndex: { stale: true, minIndex: booked.index }.minIndex });
    const asAlice = await alice.apps.view({ app, method: 'availability', args: day });
    const asOwner = await owner.apps.view({ app, method: 'availability', args: day });
    console.log('availability bob/alice/owner ->', JSON.stringify([asBob.result, asAlice.result, asOwner.result]));
    expect(JSON.stringify(asBob.result)).not.toContain('secret plan');
    expect(JSON.stringify(asAlice.result)).toContain('secret plan');
    expect(JSON.stringify(asOwner.result)).toContain('secret plan');

    const mine = await alice.apps.view({ app, method: 'myBookings', args: {} });
    expect((mine.result as Array<{ id: string }>).map((b) => b.id)).toEqual([id]);

    // Anonymous callers are refused before the app runs.
    const anon = await client(0).apps.view({ app, method: 'availability', args: day }).catch((e) => e);
    console.log('anonymous view ->', anon.status, anon.code);
    expect(anon).toBeInstanceOf(LoginRequired);
    expect((anon as LoginRequired).status).toBe(401);

    // Arguments over the node's 512 KiB are refused before the app runs.
    const huge = await alice
      .apps.call({ app, method: 'book', args: { roomId: 'r1', start: String(start + 20 * SLOT_MS), slotCount: 1, title: 'x'.repeat(600 * 1024) } })
      .catch((e) => e);
    console.log('oversized call ->', huge.status, huge.code);
    expect(huge).toBeInstanceOf(ArgsTooLarge);

    // Event audiences: Bob (org) sees SlotsTaken, never Alice's Booked.
    await until('alice: Booked', () => kinds(aliceEvents.seen).includes('Booked'));
    await until('bob: SlotsTaken', () => kinds(bobEvents.seen).includes('SlotsTaken'));
    expect(kinds(bobEvents.seen)).not.toContain('Booked');

    // adminRelease needs a passkey confirmation over the exact call; Alice is told why.
    const noConsent = await owner.apps.call({ app, method: 'adminRelease', args: { id, reason: 'maintenance' } }).catch((e) => e);
    expect(noConsent).toBeInstanceOf(ConfirmRequired);
    expect((noConsent as ConfirmRequired).code).toBe('CONFIRM_REQUIRED');
    await owner.apps.call({ app, method: 'adminRelease', args: { id, reason: 'maintenance' } }, { confirm: passkeyConfirm(ownerPk) });
    await until('alice: BookingReleased(maintenance)', () =>
      aliceEvents.seen.some((e) => e.kind === 'BookingReleased' && (e.data as { reason?: string }).reason === 'maintenance'),
    );
    const gone = await alice.apps.view({ app, method: 'booking', args: { id } }).catch((e) => e);
    expect(gone).toBeInstanceOf(AppError);
    expect((gone as AppError).appCode).toMatch(/notfound/i);

    // No-show: a booking whose start is past the 15-minute grace, never checked in,
    // is released by the app's 1-minute timer (Caller::System) with reason "no-show".
    const past = (Math.floor(Date.now() / SLOT_MS) - 2) * SLOT_MS;
    const late = await alice.apps.call({ app, method: 'book', args: { roomId: 'r1', start: String(past), slotCount: 1, title: 'standup' } });
    const lateId = late.result as string;
    await until(
      'alice: BookingReleased(no-show)',
      () => aliceEvents.seen.some((e) => e.kind === 'BookingReleased' && (e.data as { id?: string }).id === lateId),
      90_000,
    );
    console.log('no-show released', Math.round((Date.now() - t0) / 1000), 's after app creation');
    expect(bobEvents.seen.some((e) => e.kind === 'BookingReleased')).toBe(false);

    // Removal from the org cuts Bob's stream and calls off.
    await owner.orgs.removeMember({ org, account: bobId }, { confirm: passkeyConfirm(ownerPk) });
    await new Promise((r) => setTimeout(r, 300));
    const before = bobEvents.seen.length;
    await alice.apps.call({ app, method: 'book', args: { roomId: 'r1', start: String(start + 8 * SLOT_MS), slotCount: 1, title: 'after' } });
    await until('alice: second Booked', () => kinds(aliceEvents.seen).filter((k) => k === 'Booked').length >= 3);
    await new Promise((r) => setTimeout(r, 500));
    expect(bobEvents.seen.length).toBe(before);
    const cut = await bob.apps.call({ app, method: 'book', args: { roomId: 'r1', start: String(start + 12 * SLOT_MS), slotCount: 1, title: 'x' } }).catch((e) => e);
    console.log('removed member call ->', cut.status, cut.code);
    expect(cut).toBeInstanceOf(NotFound);
    expect((cut as NotFound).status).toBe(404);

    console.log('alice events', JSON.stringify(aliceEvents.seen));
    console.log('bob events', JSON.stringify(bobEvents.seen));
    console.log('accounts', { aliceId, bobId });
    await aliceEvents.stop();
    await bobEvents.stop();
  }, 180_000);
});
