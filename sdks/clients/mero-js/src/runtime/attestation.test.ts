import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it, vi } from 'vitest';

import { fromHex, hex } from '../crypto/internal.js';
import type { SignedNodeRelease } from '../sealed/release.js';
import { transportKeyBinding } from '../sealed/sealed.js';
import type { DcapCollateral, DcapVerifiedReport } from '../sealed/verify.js';
import {
  MOCK_QUOTE_HEADER,
  claimedRelease,
  fetchAttestedKey,
  freshCollateral,
  Unanswered,
  verifyNode,
  type NodePolicy,
} from './attestation.js';

// The quote's collateral source, seen from the library: no test reaches Intel or the mirror.
const dcapCalls = vi.hoisted(() => ({ getCollateral: [] as unknown[][], getCollateralFromPcs: 0, unbundled: false }));
vi.mock('@phala/dcap-qvl', () => ({
  default: undefined,
  verify: () => {
    throw new Error('each test passes its own dcapVerify');
  },
  getCollateral: async (...args: unknown[]) => {
    dcapCalls.getCollateral.push(args);
    if (args[0] === 'http://down.test') throw new TypeError('fetch failed');
    if (args[0] === 'http://bad.test') throw new Error('Failed to parse TCB info');
    return {} as DcapCollateral;
  },
  get getCollateralFromPcs() {
    if (dcapCalls.unbundled) return undefined;
    return async () => {
      dcapCalls.getCollateralFromPcs += 1;
      return {} as DcapCollateral;
    };
  },
}));

// Real mero-tee releases (Sigstore-signed by the node release workflow), so the
// release half of every case below is the real check, not a stub.
const fixture = (name: string) =>
  readFileSync(fileURLToPath(new URL(`../sealed/fixtures/${name}`, import.meta.url)), 'utf8');
const signed = (version: string): SignedNodeRelease => ({
  version,
  publishedMrtds: fixture(`mero-tee-v${version}.published-mrtds.json`),
  bundle: fixture(`mero-tee-v${version}.published-mrtds.json.bundle.json`),
});
const R87 = signed('2.3.87');
const PROFILES = JSON.parse(R87.publishedMrtds).profiles as Record<string, Record<string, string>>;
const IMAGE = PROFILES['locked-read-only'];
const DEBUG_IMAGE = PROFILES.debug;
const KMS_IDENTITY =
  'https://github.com/calimero-network/mero-tee/.github/workflows/release-kms.yaml@refs/heads/master';

const BASE = 'http://node.test';
const TRANSPORT_KEY = new Uint8Array(32).fill(5);
const REAL_QUOTE = btoa('a real TDX quote, as far as this test is concerned');

interface Td {
  image?: Record<string, string>;
  status?: string;
  debug?: boolean;
  /** Bind this nonce instead of the one asked for (a replayed quote). */
  nonce?: string;
  /** Bind this transport key instead of the one served. */
  boundKey?: Uint8Array;
}

/**
 * A node at `BASE` serving an attestation doc, and the DCAP verifier stand-in
 * for its quote: it answers as `@phala/dcap-qvl` would for a genuine quote from
 * the TD described by `td`, binding what `td` says.
 */
function node(td: Td = {}, opts: { claimed?: string; quote?: string; app?: Uint8Array } = {}) {
  let asked = '';
  const queries: URLSearchParams[] = [];
  const fetchImpl = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    asked = url.searchParams.get('nonce') ?? '';
    queries.push(url.searchParams);
    return new Response(
      JSON.stringify({
        quote: opts.quote ?? REAL_QUOTE,
        transportKey: hex(TRANSPORT_KEY),
        appHash: hex(opts.app ?? new Uint8Array(32)),
        nonce: asked,
        release: { version: opts.claimed ?? '2.3.87', profile: 'locked-read-only' },
        cluster: 'test',
        node: 1,
        mock: false,
      }),
    );
  }) as typeof fetch;
  const verify = async (): Promise<DcapVerifiedReport> => {
    const image = td.image ?? IMAGE;
    const binding = await transportKeyBinding(opts.app ?? new Uint8Array(32), td.boundKey ?? TRANSPORT_KEY);
    const reportData = new Uint8Array(64);
    reportData.set(fromHex(td.nonce ?? asked, 'nonce', 32));
    reportData.set(binding, 32);
    const attrs = new Uint8Array(8);
    if (td.debug) attrs[0] = 1;
    return {
      status: td.status ?? 'UpToDate',
      report: {
        type: 'td10',
        data: {
          mrTd: fromHex(image.mrtd, 'mrtd', 48),
          rtMr0: fromHex(image.rtmr0, 'rtmr0', 48),
          rtMr1: fromHex(image.rtmr1, 'rtmr1', 48),
          rtMr2: fromHex(image.rtmr2, 'rtmr2', 48),
          rtMr3: fromHex(image.rtmr3, 'rtmr3', 48),
          reportData,
          tdAttributes: attrs,
        },
      },
    };
  };
  return { fetchImpl, verify, queries };
}

/** The production policy, with every network edge pointed at the fixtures. */
async function attest(
  td: Td,
  release: Partial<NonNullable<NodePolicy['release']>> = {},
  opts: { claimed?: string; quote?: string; app?: Uint8Array; policy?: Partial<NodePolicy>; verify?: boolean } = {},
) {
  const n = node(td, opts);
  // dcap-qvl's verify is synchronous: the report is computed once the doc is served.
  let report: DcapVerifiedReport | undefined;
  const releaseSource = vi.fn(async (version: string) => (version === '2.3.87' ? R87 : signed(version)));
  const policy: NodePolicy = {
    release: {
      minReleaseVersion: '2.3.87',
      releaseSource,
      dcapVerify: () => {
        if (!report) throw new Error('no report');
        return report;
      },
      fetchCollateral: async () => ({}) as DcapCollateral,
      ...release,
    },
    ...opts.policy,
  };
  const wrapped = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const response = await n.fetchImpl(input, init);
    report = await n.verify();
    return response;
  }) as typeof fetch;
  const result = opts.verify ? verifyNode(BASE, policy, wrapped) : fetchAttestedKey(BASE, policy, wrapped);
  return { result, releaseSource, queries: n.queries };
}

describe('fetchAttestedKey with a release policy (core Measurements::Release)', () => {
  it('accepts a genuine quote from the signed release image, bound to this nonce and key', async () => {
    const { result } = await attest({});
    await expect(result).resolves.toEqual(TRANSPORT_KEY);
  });

  it('1. refuses a mock quote unless allowMock is set', async () => {
    const mock = btoa(MOCK_QUOTE_HEADER + '\0'.repeat(64));
    const { result } = await attest({}, {}, { quote: mock });
    await expect(result).rejects.toThrow(/mock quote refused/);
  });

  it('2. refuses a real quote when no image is trusted', async () => {
    const { result } = await attest({}, {}, { policy: { release: undefined } });
    await expect(result).rejects.toThrow(/trusts no TD image/);
  });

  it('3. refuses a release below the floor, before fetching anything', async () => {
    const { result, releaseSource } = await attest({}, { minReleaseVersion: '2.3.88' });
    await expect(result).rejects.toThrow(/older than the minimum 2.3.88/);
    expect(releaseSource).not.toHaveBeenCalled();
  });

  it('4. refuses a version the node cannot name', async () => {
    const { result } = await attest({}, {}, { claimed: 'latest' });
    await expect(result).rejects.toThrow(/not a release version/);
  });

  it('5. refuses a signed release that is not the one the node names', async () => {
    const { result } = await attest({}, { minReleaseVersion: '2.3.86', releaseSource: async () => ({ ...signed('2.3.86'), version: '2.3.87' }) });
    await expect(result).rejects.toThrow(/2\.3\.86, not 2\.3\.87/);
  });

  it('5b. refuses a genuinely signed release other than the one the node names', async () => {
    const image = JSON.parse(signed('2.3.86').publishedMrtds).profiles['locked-read-only'];
    const { result } = await attest({ image }, { minReleaseVersion: '2.3.86', releaseSource: async () => signed('2.3.86') });
    await expect(result).rejects.toThrow(/names mero-tee release 2\.3\.87.*is 2\.3\.86/);
  });

  it('6. refuses a release whose measurements file was tampered with', async () => {
    const tampered = { ...R87, publishedMrtds: fixture('mero-tee-v2.3.87.tampered.published-mrtds.json') };
    const { result } = await attest({}, { releaseSource: async () => tampered });
    await expect(result).rejects.toThrow(/not the one signed/);
  });

  it('7. refuses a release signed by another workflow than the pinned one', async () => {
    const { result } = await attest({}, { identity: KMS_IDENTITY });
    await expect(result).rejects.toThrow(/identity|release-kms/i);
  });

  it('8. refuses a quote from an image the release does not list', async () => {
    const { result } = await attest({ image: { ...IMAGE, rtmr1: 'ab'.repeat(48) } });
    await expect(result).rejects.toThrow(/not an image this verifier trusts/);
  });

  it('9. refuses the debug profile image unless a debug profile is allowed', async () => {
    const { result } = await attest({ image: DEBUG_IMAGE });
    await expect(result).rejects.toThrow(/not an image this verifier trusts/);
    const { result: asked } = await attest({ image: DEBUG_IMAGE }, { profile: 'debug' });
    await expect(asked).rejects.toThrow(/not a production profile/);
  });

  it('10. refuses a debug TD', async () => {
    const { result } = await attest({ debug: true });
    await expect(result).rejects.toThrow(/debug TD/);
  });

  it('11. refuses a TCB status the release does not accept, and Revoked always', async () => {
    const { result } = await attest({ status: 'SWHardeningNeeded' });
    await expect(result).rejects.toThrow(/TCB status is SWHardeningNeeded/);
    const { result: revoked } = await attest({ status: 'Revoked' });
    await expect(revoked).rejects.toThrow(/TCB status is Revoked/);
  });

  it('12. refuses a replayed quote (bound to another nonce)', async () => {
    const { result } = await attest({ nonce: '11'.repeat(32) });
    await expect(result).rejects.toThrow(/does not commit to this nonce/);
  });

  it('13. refuses a quote that binds another transport key', async () => {
    const { result } = await attest({ boundKey: new Uint8Array(32).fill(6) });
    await expect(result).rejects.toThrow(/does not commit to this nonce and binding/);
  });
});

describe('claimedRelease', () => {
  it('reads the tag form and refuses what is below the floor', () => {
    expect(claimedRelease({ release: { version: 'mero-tee-v2.3.109', profile: '' } }, '2.3.100')).toBe('2.3.109');
    expect(() => claimedRelease({ release: { version: '2.3.99', profile: '' } }, '2.3.100')).toThrow(/older/);
    expect(() => claimedRelease({ release: { version: '2.3.109', profile: '' } }, 'latest')).toThrow(/minReleaseVersion/);
  });
});

describe("the quote's collateral", () => {
  it("comes from the cloud's mirror by default, which browsers can read; Intel's PCS only on request", async () => {
    dcapCalls.getCollateral.length = 0;
    dcapCalls.getCollateralFromPcs = 0;
    await expect((await attest({}, { fetchCollateral: undefined })).result).resolves.toEqual(TRANSPORT_KEY);
    expect(dcapCalls.getCollateral.map((a) => a[0])).toEqual(['https://cloud.calimero.network/api/tee/pcs']);
    expect(dcapCalls.getCollateralFromPcs).toBe(0);

    await expect(
      (await attest({}, { fetchCollateral: undefined, collateralSource: 'https://pccs.example' })).result,
    ).resolves.toEqual(TRANSPORT_KEY);
    expect(dcapCalls.getCollateral.at(-1)?.[0]).toBe('https://pccs.example');

    await expect((await attest({}, { fetchCollateral: undefined, collateralSource: 'intel' })).result).resolves.toEqual(
      TRANSPORT_KEY,
    );
    expect(dcapCalls.getCollateralFromPcs).toBe(1);
  });

  it('is refused when older than maxCollateralAgeDays, and only then', async () => {
    const now = Date.parse('2026-10-30T12:00:00Z');
    const issued = (iso: string) => async () => ({ tcb_info: JSON.stringify({ issueDate: iso }) }) as DcapCollateral;
    const fresh = freshCollateral(issued('2026-10-28T00:00:00Z'), { maxCollateralAgeDays: 7, now: () => now });
    await expect(fresh(new Uint8Array())).resolves.toBeTruthy();
    const stale = freshCollateral(issued('2026-10-01T00:00:00Z'), { maxCollateralAgeDays: 7, now: () => now });
    await expect(stale(new Uint8Array())).rejects.toThrow(/issued 29 days ago, more than the 7 this client accepts/);
    const undated = freshCollateral(async () => ({ tcb_info: '{}' }) as DcapCollateral, { maxCollateralAgeDays: 7 });
    await expect(undated(new Uint8Array())).rejects.toThrow(/no TCB info issue date/);
    // Unset: the fetch itself, untouched (Intel's nextUpdate is checked by dcap-qvl).
    const fetch = issued('2020-01-01T00:00:00Z');
    expect(freshCollateral(fetch, {})).toBe(fetch);
  });

  it('is unanswered when its source cannot be reached, and refused when it answers something unusable', async () => {
    const down = (await attest({}, { fetchCollateral: undefined, collateralSource: 'http://down.test' })).result;
    await expect(down).rejects.toBeInstanceOf(Unanswered);
    await expect(down).rejects.toMatchObject({ host: 'http://down.test', status: undefined });
    const bad = (await attest({}, { fetchCollateral: undefined, collateralSource: 'http://bad.test' })).result;
    await expect(bad).rejects.toThrow(/Failed to parse TCB info/);
    await expect(bad).rejects.not.toBeInstanceOf(Unanswered);
  });

  it('is refused, not unanswered, for a collateral source that is not a URL', async () => {
    const calls = dcapCalls.getCollateral.length;
    const { result } = await attest({}, { fetchCollateral: undefined, collateralSource: 'not a url' });
    await expect(result).rejects.toThrow(TypeError);
    await expect(result).rejects.not.toBeInstanceOf(Unanswered);
    expect(dcapCalls.getCollateral.length).toBe(calls);
  });

  it('is refused, not unanswered, when the bundle carries no collateral fetch', async () => {
    dcapCalls.unbundled = true;
    try {
      const { result } = await attest({}, { fetchCollateral: undefined, collateralSource: 'intel' });
      await expect(result).rejects.toThrow(/loaded without getCollateralFromPcs/);
      await expect(result).rejects.not.toBeInstanceOf(Unanswered);
    } finally {
      dcapCalls.unbundled = false;
    }
  });

  it('is checked for age on the path a node is attested through', async () => {
    const stale = async () => ({ tcb_info: JSON.stringify({ issueDate: '2020-01-01T00:00:00Z' }) }) as DcapCollateral;
    const { result } = await attest({}, { fetchCollateral: stale, maxCollateralAgeDays: 30 });
    await expect(result).rejects.toThrow(/more than the 30 this client accepts/);
  });
});

/** A node serving a mock quote that binds `app` (zeros by default), the nonce and the transport key. */
function mockNode(app: Uint8Array = new Uint8Array(32)): typeof fetch {
  return (async (input: RequestInfo | URL) => {
    const nonce = new URL(String(input)).searchParams.get('nonce') ?? '';
    const suffix = await transportKeyBinding(app, TRANSPORT_KEY);
    const report = new Uint8Array(64);
    report.set(fromHex(nonce, 'nonce', 32));
    report.set(suffix, 32);
    const quote = new Uint8Array([...new TextEncoder().encode(MOCK_QUOTE_HEADER), ...report]);
    return new Response(
      JSON.stringify({
        quote: btoa(String.fromCharCode(...quote)),
        transportKey: hex(TRANSPORT_KEY),
        appHash: hex(app),
        release: { version: '9.9.9', profile: 'debug' },
        cluster: 'claimed',
        mock: true,
      }),
    );
  }) as typeof fetch;
}

describe('verifyNode: the record of what was verified', () => {
  const verified = async (td: Td = {}, release = {}, opts = {}) => {
    const { result } = await attest(td, release, { ...opts, verify: true });
    return (await result) as Awaited<ReturnType<typeof verifyNode>>;
  };

  it('names the release and profile the quote was checked against, with the transport key it bound', async () => {
    const before = Date.now();
    const { key, verification } = await verified();
    expect(key).toEqual(TRANSPORT_KEY);
    expect(verification).toEqual({
      url: BASE,
      mock: false,
      release: '2.3.87',
      profile: 'locked-read-only',
      checkedAt: expect.any(Number),
    });
    expect(verification.checkedAt).toBeGreaterThanOrEqual(before);
  });

  it('names the debug profile only when it was allowed and matched', async () => {
    const { verification } = await verified({ image: DEBUG_IMAGE }, { profile: 'debug', allowDebugProfile: true });
    expect(verification.profile).toBe('debug');
  });

  it('leaves the app hash out when the policy only pins one, which the node merely echoes', async () => {
    const policy: NodePolicy = { appHash: 'ab'.repeat(32), verifyQuote: async () => true };
    const n = node({ boundKey: TRANSPORT_KEY });
    const { verification } = await verifyNode(BASE, policy, (async (input: RequestInfo | URL) => n.fetchImpl(input)) as typeof fetch);
    expect(verification.appHash).toBeUndefined();
    expect(n.queries[0].get('appHash')).toBe('ab'.repeat(32));
    expect(n.queries[0].has('app')).toBe(false);
  });

  it('marks a mock attestation, with no release or profile', async () => {
    const { verification } = await verifyNode(BASE, { allowMock: true }, mockNode());
    expect(verification).toEqual({ url: BASE, mock: true, checkedAt: expect.any(Number) });
  });

  it('is not produced for a node that fails verification', async () => {
    await expect(verified({ debug: true })).rejects.toThrow(/debug TD/);
  });
});

describe('an app-bound attestation (the node binds its own record of the app)', () => {
  const APP_HASH = new Uint8Array(32).fill(0xcd);
  const bound = async (app: Uint8Array | undefined, policy: Partial<NodePolicy> = {}) => {
    const { result, queries } = await attest({}, {}, { app, verify: true, policy: { app: 'app-1', ...policy } });
    return { result: result as ReturnType<typeof verifyNode>, queries };
  };

  it('records the app hash the node bound, asking by app id and not by hash', async () => {
    const { result, queries } = await bound(APP_HASH, { appHash: 'CD'.repeat(32) });
    expect((await result).verification.appHash).toBe(hex(APP_HASH));
    expect(queries[0].get('app')).toBe('app-1');
    expect(queries[0].has('appHash')).toBe(false);
  });

  it('refuses an all-zero app hash: the node bound no app', async () => {
    const { result } = await bound(undefined);
    await expect(result).rejects.toThrow(/all zero/);
  });

  it('refuses an app hash other than the one the policy pinned', async () => {
    const { result } = await bound(APP_HASH, { appHash: 'ab'.repeat(32) });
    await expect(result).rejects.toThrow(/not the pinned/);
  });

  it('refuses a quote that does not bind the app hash the node names', async () => {
    const n = node({}, { app: APP_HASH });
    const lying = (async (input: RequestInfo | URL) => {
      const doc = await (await n.fetchImpl(input)).json();
      return new Response(JSON.stringify({ ...doc, appHash: hex(new Uint8Array(32).fill(1)) }));
    }) as typeof fetch;
    const expected = hex(await transportKeyBinding(APP_HASH, TRANSPORT_KEY));
    const policy: NodePolicy = { app: 'app-1', verifyQuote: async (args) => args.reportDataSuffix === expected };
    await expect(verifyNode(BASE, policy, lying)).rejects.toThrow(/did not verify/);
  });

  it('records the bound app hash of a mock attestation too, marked as mock', async () => {
    const { verification } = await verifyNode(BASE, { allowMock: true, app: 'app-1' }, mockNode(APP_HASH));
    expect(verification).toMatchObject({ mock: true, appHash: hex(APP_HASH) });
  });
});
