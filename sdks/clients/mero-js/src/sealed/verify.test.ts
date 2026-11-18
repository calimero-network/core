import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { verify as dcapVerify } from '@phala/dcap-qvl';
import { describe, expect, it, vi } from 'vitest';

import { hex } from '../crypto/internal.js';
import { transportKeyBinding } from './sealed.js';
import {
  createQuoteVerifier,
  trustedMeasurementsFromReleases,
  type DcapCollateral,
  type DcapVerifiedReport,
  type PublishedMrtds,
} from './verify.js';

// A real TDX quote and the Intel-signed collateral for it, from dcap-qvl's own
// samples, as core's calimero-tee-attestation tests them: the collateral is in
// the JSON form a node returns with `includeCollateral`.
const fixture = (name: string) => fileURLToPath(new URL(`./fixtures/${name}`, import.meta.url));
const QUOTE = new Uint8Array(readFileSync(fixture('tdx_quote.bin')));
const COLLATERAL = JSON.parse(readFileSync(fixture('tdx_quote_collateral.json'), 'utf8')) as DcapCollateral;
// A moment inside that collateral's validity window (2025-06-27).
const AT = 1_751_000_000_000;
const QUOTE_B64 = Buffer.from(QUOTE).toString('base64');

// The sample quote's own report data and MRTD, read once from the verified
// report, so the tests below say what they check rather than repeat hex.
const sample = dcapVerify(QUOTE, COLLATERAL, AT / 1000);
const sampleTd = sample.report.data as {
  mrTd: Uint8Array;
  reportData: Uint8Array;
  rtMr0: Uint8Array;
  rtMr1: Uint8Array;
  rtMr2: Uint8Array;
  rtMr3: Uint8Array;
};
const MRTD = hex(sampleTd.mrTd);
// The sample's image: every register, as a release's published-mrtds.json lists it.
const IMAGE = {
  mrtd: MRTD,
  rtmr0: hex(sampleTd.rtMr0),
  rtmr1: hex(sampleTd.rtMr1),
  rtmr2: hex(sampleTd.rtMr2),
  rtmr3: hex(sampleTd.rtMr3),
};
const NONCE = hex(sampleTd.reportData.slice(0, 32));
const SUFFIX = hex(sampleTd.reportData.slice(32));

const real = (overrides: Partial<Parameters<typeof createQuoteVerifier>[0]> = {}) =>
  createQuoteVerifier({ dcapVerify, allowedMeasurements: [IMAGE], now: () => AT, ...overrides });

describe('createQuoteVerifier on a real quote', () => {
  it('accepts a genuine quote from a trusted image that commits to the binding', async () => {
    const verifier = real();
    await expect(
      verifier({ quoteB64: QUOTE_B64, nonce: NONCE, reportDataSuffix: SUFFIX, collateral: COLLATERAL }),
    ).resolves.toBe(true);
    expect(verifier.includeCollateral).toBe(true);
  });

  it('refuses a quote that commits to another binding', async () => {
    await expect(
      real()({ quoteB64: QUOTE_B64, nonce: NONCE, reportDataSuffix: '00'.repeat(32), collateral: COLLATERAL }),
    ).rejects.toThrow('does not commit to this nonce and binding');
  });

  it('refuses an image it does not trust', async () => {
    await expect(
      real({ allowedMeasurements: [{ ...IMAGE, rtmr3: 'ab'.repeat(48) }] })({
        quoteB64: QUOTE_B64,
        nonce: NONCE,
        reportDataSuffix: SUFFIX,
        collateral: COLLATERAL,
      }),
    ).rejects.toThrow('are not an image this verifier trusts');
  });

  it('never combines the registers of different images', async () => {
    const args = { quoteB64: QUOTE_B64, nonce: NONCE, reportDataSuffix: SUFFIX, collateral: COLLATERAL };
    const other = 'ab'.repeat(48);
    // Each of the sample's registers is trusted, but each by a different image.
    const split = real({
      allowedMeasurements: [
        { ...IMAGE, rtmr1: other },
        { ...IMAGE, rtmr2: other },
        { ...IMAGE, rtmr3: other },
      ],
    });
    await expect(split(args)).rejects.toThrow('are not an image this verifier trusts');
    await expect(real({ allowedMeasurements: [{ ...IMAGE, rtmr1: other }, IMAGE] })(args)).resolves.toBe(true);
  });

  it('accepts MRTD allowlists only together with the image registers', async () => {
    const args = { quoteB64: QUOTE_B64, nonce: NONCE, reportDataSuffix: SUFFIX, collateral: COLLATERAL };
    const lists = {
      allowedMeasurements: undefined,
      allowedMrtd: [MRTD],
      allowedRtmr1: [IMAGE.rtmr1],
      allowedRtmr2: [IMAGE.rtmr2],
      allowedRtmr3: [IMAGE.rtmr3],
    };
    await expect(real(lists)(args)).resolves.toBe(true);
    await expect(real({ ...lists, allowedMrtd: ['ab'.repeat(48)] })(args)).rejects.toThrow('MRTD');
  });

  it('checks an RTMR only when asked to', async () => {
    const args = { quoteB64: QUOTE_B64, nonce: NONCE, reportDataSuffix: SUFFIX, collateral: COLLATERAL };
    await expect(real({ allowedRtmr0: [hex(sampleTd.rtMr0)] })(args)).resolves.toBe(true);
    await expect(real({ allowedRtmr0: ['cd'.repeat(48)] })(args)).rejects.toThrow('RTMR0');
  });

  it('refuses a tampered quote', async () => {
    const tampered = QUOTE.slice();
    tampered[200] ^= 0x01; // a byte of the signed TD report body
    await expect(
      real()({
        quoteB64: Buffer.from(tampered).toString('base64'),
        nonce: NONCE,
        reportDataSuffix: SUFFIX,
        collateral: COLLATERAL,
      }),
    ).rejects.toThrow('did not verify');
  });

  it('refuses collateral outside its validity window', async () => {
    await expect(
      real({ now: () => Date.UTC(2040, 0, 1) })({
        quoteB64: QUOTE_B64,
        nonce: NONCE,
        reportDataSuffix: SUFFIX,
        collateral: COLLATERAL,
      }),
    ).rejects.toThrow('did not verify');
  });

  it('gets collateral itself when the node sends none, and refuses without a way to', async () => {
    const fetchCollateral = vi.fn(async () => COLLATERAL);
    const args = { quoteB64: QUOTE_B64, nonce: NONCE, reportDataSuffix: SUFFIX };
    await expect(real({ fetchCollateral })(args)).resolves.toBe(true);
    expect(fetchCollateral).toHaveBeenCalledWith(QUOTE);
    await expect(real()(args)).rejects.toThrow('no collateral');
  });
});

describe('createQuoteVerifier policy', () => {
  const report = (status: string, type = 'td10'): DcapVerifiedReport => ({
    status,
    report: { type, data: sampleTd },
  });
  const args = { quoteB64: QUOTE_B64, nonce: NONCE, reportDataSuffix: SUFFIX, collateral: COLLATERAL };

  it('accepts only up-to-date platforms by default', async () => {
    const outOfDate = createQuoteVerifier({ dcapVerify: () => report('OutOfDate'), allowedMeasurements: [IMAGE] });
    await expect(outOfDate(args)).rejects.toThrow('TCB status is OutOfDate');
    const allowed = createQuoteVerifier({
      dcapVerify: () => report('OutOfDate'),
      allowedMeasurements: [IMAGE],
      allowedTcbStatuses: ['UpToDate', 'OutOfDate'],
    });
    await expect(allowed(args)).resolves.toBe(true);
  });

  it('refuses a revoked platform even when told to accept it', async () => {
    const verifier = createQuoteVerifier({
      dcapVerify: () => report('Revoked'),
      allowedMeasurements: [IMAGE],
      allowedTcbStatuses: ['Revoked'],
    });
    await expect(verifier(args)).rejects.toThrow('Revoked');
  });

  // Whatever dcapVerify was given: a debug TD's memory and registers are open
  // to the host, so its measurements prove nothing about what runs in it.
  it('refuses a debug TD, or one whose attributes it cannot read, whatever dcapVerify allows', async () => {
    const withAttributes = (tdAttributes: Uint8Array | undefined, type = 'td10'): DcapVerifiedReport => {
      const td = { ...sampleTd, tdAttributes };
      return { status: 'UpToDate', report: { type, data: type === 'td15' ? { base: td } : td } };
    };
    const verifierFor = (r: DcapVerifiedReport) => createQuoteVerifier({ dcapVerify: () => r, allowedMeasurements: [IMAGE] });

    const debug = new Uint8Array(8);
    debug[0] = 0x01;
    await expect(verifierFor(withAttributes(debug))(args)).rejects.toThrow('debug TD');
    await expect(verifierFor(withAttributes(debug, 'td15'))(args)).rejects.toThrow('debug TD');
    await expect(verifierFor(withAttributes(undefined))(args)).rejects.toThrow('debug TD');
    await expect(verifierFor(withAttributes(new Uint8Array(4)))(args)).rejects.toThrow('debug TD');
    await expect(verifierFor(withAttributes(new Uint8Array(8)))(args)).resolves.toBe(true);
  });

  it('refuses a quote that is not TDX', async () => {
    const verifier = createQuoteVerifier({ dcapVerify: () => report('UpToDate', 'sgx'), allowedMeasurements: [IMAGE] });
    await expect(verifier(args)).rejects.toThrow('not a TDX one');
  });

  it('passes the time now, in seconds', async () => {
    const dcap = vi.fn(() => report('UpToDate'));
    await createQuoteVerifier({ dcapVerify: dcap, allowedMeasurements: [IMAGE], now: () => 1_234_567_890 })(args);
    expect(dcap).toHaveBeenCalledWith(QUOTE, COLLATERAL, 1_234_567);
  });

  it('will not be built to accept nothing, or with a measurement that is not one', () => {
    expect(() => createQuoteVerifier({ dcapVerify })).toThrow('No image is trusted');
    expect(() => createQuoteVerifier({ dcapVerify, allowedMeasurements: [] })).toThrow(
      'allowedMeasurements is empty',
    );
    expect(() => createQuoteVerifier({ dcapVerify, allowedMeasurements: [{ ...IMAGE, rtmr2: 'aabb' }] })).toThrow(
      'allowedMeasurements[0].rtmr2',
    );
    expect(() =>
      createQuoteVerifier({ dcapVerify, allowedMeasurements: [IMAGE], allowedRtmr2: ['zz'] }),
    ).toThrow('allowedRtmr2');
  });

  it('will not trust an MRTD alone, which names the firmware and not the image', () => {
    expect(() => createQuoteVerifier({ dcapVerify, allowedMrtd: [MRTD] })).toThrow('does not name an image');
    expect(() =>
      createQuoteVerifier({ dcapVerify, allowedMrtd: [MRTD], allowedRtmr1: [IMAGE.rtmr1], allowedRtmr2: [IMAGE.rtmr2] }),
    ).toThrow('does not name an image');
  });
});

describe('trustedMeasurementsFromReleases', () => {
  // The shape of a mero-tee release's published-mrtds.json, cut to what is read.
  const release = (tag: string, rtmr3: string, statuses = ['uptodate', 'outofdate']): PublishedMrtds => ({
    role: 'node',
    tag,
    profiles: {
      'locked-read-only': { ...IMAGE, rtmr3, allowed_tcb_statuses: statuses },
      debug: { ...IMAGE, rtmr3: 'dd'.repeat(48), allowed_tcb_statuses: statuses },
    },
  });

  it("trusts each release's image of one profile, by all of its measurements", () => {
    const options = trustedMeasurementsFromReleases([release('2.3.76', 'aa'.repeat(48)), release('2.3.78', IMAGE.rtmr3)], {
      profile: 'locked-read-only',
    });
    expect(options.allowedMeasurements).toEqual([{ ...IMAGE, rtmr3: 'aa'.repeat(48) }, IMAGE]);
    expect(options.allowedTcbStatuses).toEqual(['UpToDate', 'OutOfDate']);
  });

  it('builds a verifier that accepts the release image and not another profile of it', async () => {
    const args = { quoteB64: QUOTE_B64, nonce: NONCE, reportDataSuffix: SUFFIX, collateral: COLLATERAL };
    const locked = trustedMeasurementsFromReleases([release('2.3.78', IMAGE.rtmr3)], { profile: 'locked-read-only' });
    await expect(createQuoteVerifier({ dcapVerify, now: () => AT, ...locked })(args)).resolves.toBe(true);
    const debug = trustedMeasurementsFromReleases([release('2.3.78', IMAGE.rtmr3)], {
      profile: 'debug',
      allowDebugProfile: true,
    });
    await expect(createQuoteVerifier({ dcapVerify, now: () => AT, ...debug })(args)).rejects.toThrow(
      'are not an image this verifier trusts',
    );
  });

  // A debug profile's image runs with SSH and the operator's tooling inside the
  // TD, so its measurements name an image the operator can read; only
  // locked-read-only is a production image.
  it('trusts only the production profile unless debug profiles are asked for', () => {
    const debugRelease = {
      ...release('2.3.78', IMAGE.rtmr3),
      profiles: {
        ...release('2.3.78', IMAGE.rtmr3).profiles,
        'debug-read-only': { ...IMAGE, rtmr3: 'ee'.repeat(48), allowed_tcb_statuses: ['uptodate'] },
      },
    };
    for (const profile of ['debug', 'debug-read-only']) {
      expect(() => trustedMeasurementsFromReleases([debugRelease], { profile })).toThrow(
        `${JSON.stringify(profile)} is not a production profile`,
      );
      expect(() =>
        trustedMeasurementsFromReleases([debugRelease], { profile, allowDebugProfile: false }),
      ).toThrow('not a production profile');
      expect(
        trustedMeasurementsFromReleases([debugRelease], { profile, allowDebugProfile: true }).allowedMeasurements,
      ).toHaveLength(1);
    }
    expect(
      trustedMeasurementsFromReleases([debugRelease], { profile: 'locked-read-only' }).allowedMeasurements,
    ).toEqual([IMAGE]);
  });

  it('accepts only the TCB statuses every release accepts', () => {
    const options = trustedMeasurementsFromReleases(
      [release('2.3.76', IMAGE.rtmr3, ['uptodate']), release('2.3.78', IMAGE.rtmr3)],
      { profile: 'locked-read-only' },
    );
    expect(options.allowedTcbStatuses).toEqual(['UpToDate']);
  });

  it('refuses what it cannot read as a node release', () => {
    expect(() => trustedMeasurementsFromReleases([], { profile: 'locked-read-only' })).toThrow('No release');
    expect(() => trustedMeasurementsFromReleases([release('2.3.78', IMAGE.rtmr3)], { profile: 'prod' })).toThrow(
      'has no profile "prod"',
    );
    expect(() =>
      trustedMeasurementsFromReleases([{ ...release('2.3.78', IMAGE.rtmr3), role: 'kms' }], {
        profile: 'locked-read-only',
      }),
    ).toThrow('not of a node image');
    expect(() =>
      trustedMeasurementsFromReleases([release('2.3.78', IMAGE.rtmr3, ['sometimes'])], { profile: 'locked-read-only' }),
    ).toThrow('does not know');
    expect(() =>
      trustedMeasurementsFromReleases(
        [release('2.3.76', IMAGE.rtmr3, ['uptodate']), release('2.3.78', IMAGE.rtmr3, ['outofdate'])],
        { profile: 'locked-read-only' },
      ),
    ).toThrow('no TCB status in common');
  });
});
