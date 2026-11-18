import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { verify as dcapVerify } from '@phala/dcap-qvl';
import { describe, expect, it, vi } from 'vitest';

import { hex } from '../crypto/internal.js';
import {
  DEFAULT_RELEASE_MIRROR,
  NODE_RELEASE_SIGNER,
  cloudNodeReleaseUrl,
  createSignedReleaseVerifier,
  fetchNodeRelease,
  trustSignedRelease,
  verifySignedNodeRelease,
  type SignedNodeRelease,
} from './release.js';
import { chainsToFulcio } from './sigstore.js';
import { FULCIO_AUTHORITIES, REKOR_LOGS } from './sigstore-trust-root.js';
import type { DcapCollateral } from './verify.js';
import { fromBase64, parseCertificate, pemCertificate, verifyEcdsa } from './x509.js';

// Real mero-tee releases, as the release workflow published them: each
// release's published-mrtds.json, its cosign bundle and its detached .sig.
const fixture = (name: string) => fileURLToPath(new URL(`./fixtures/${name}`, import.meta.url));
const text = (name: string) => readFileSync(fixture(name), 'utf8');
const release = (version: string) => ({
  publishedMrtds: text(`mero-tee-v${version}.published-mrtds.json`),
  bundle: text(`mero-tee-v${version}.published-mrtds.json.bundle.json`),
  signature: text(`mero-tee-v${version}.published-mrtds.json.sig`),
});
const R86 = release('2.3.86');
const R87 = release('2.3.87');
/** 2.3.87's published-mrtds.json with one byte changed. */
const TAMPERED = text('mero-tee-v2.3.87.tampered.published-mrtds.json');
/** A file the same workflow signed in 2.3.87 that is not the measurements. */
const PROVENANCE = {
  body: text('mero-tee-v2.3.87.release-provenance.json'),
  bundle: text('mero-tee-v2.3.87.release-provenance.json.bundle.json'),
};
/** mero-kms 2.3.87's attestation policy: `role: "kms"`, signed by the KMS release workflow. */
const KMS = {
  body: text('mero-kms-v2.3.87.kms-attestation-policy.json'),
  bundle: text('mero-kms-v2.3.87.kms-attestation-policy.json.bundle.json'),
};
const KMS_IDENTITY =
  'https://github.com/calimero-network/mero-tee/.github/workflows/release-kms.yaml@refs/heads/master';

const signed = (version: string): SignedNodeRelease => {
  const { publishedMrtds, bundle } = release(version);
  return { version, publishedMrtds, bundle };
};

/** The 2.3.87 bundle with `edit` applied to its parsed JSON. */
const edited = (edit: (bundle: Record<string, any>) => void): string => {
  const bundle = JSON.parse(R87.bundle);
  edit(bundle);
  return JSON.stringify(bundle);
};
const leafDer = (bundle: string) => pemCertificate(atob(JSON.parse(bundle).cert), 'cert');
const toPem = (der: Uint8Array) =>
  `-----BEGIN CERTIFICATE-----\n${Buffer.from(der).toString('base64')}\n-----END CERTIFICATE-----\n`;
/** A base64 value with its decoded byte `index` flipped. */
const flipBase64 = (value: string, index: number) => {
  const bytes = Buffer.from(value, 'base64');
  bytes[index] ^= 0x01;
  return bytes.toString('base64');
};

describe('verifySignedNodeRelease on real releases', () => {
  it('verifies 2.3.86 and 2.3.87, from text or bytes, bundle as text or JSON', async () => {
    const r86 = await verifySignedNodeRelease(R86.publishedMrtds, R86.bundle, { signature: R86.signature });
    expect(r86).toMatchObject({ role: 'node', tag: '2.3.86' });
    const r87 = await verifySignedNodeRelease(new TextEncoder().encode(R87.publishedMrtds), JSON.parse(R87.bundle), {
      expectedVersion: '2.3.87',
      signature: R87.signature,
    });
    expect(Object.keys(r87.profiles)).toEqual(expect.arrayContaining(['debug', 'locked-read-only']));
    expect(r87.profiles['locked-read-only'].rtmr3).toMatch(/^[0-9a-f]{96}$/);
  });

  it('refuses a file with one byte changed', async () => {
    await expect(verifySignedNodeRelease(TAMPERED, R87.bundle)).rejects.toThrow(
      "The file's SHA-256 is not the one Rekor logged",
    );
  });

  it("refuses one release's file with another's bundle", async () => {
    await expect(verifySignedNodeRelease(R87.publishedMrtds, R86.bundle)).rejects.toThrow(
      "The file's SHA-256 is not the one Rekor logged",
    );
  });

  it('refuses a signer other than the one pinned', async () => {
    await expect(verifySignedNodeRelease(R87.publishedMrtds, R87.bundle, { identity: KMS_IDENTITY })).rejects.toThrow(
      `The release was signed by "${NODE_RELEASE_SIGNER.identity}", not by ${KMS_IDENTITY}`,
    );
    // A genuine file genuinely signed by the KMS workflow is not a node release.
    await expect(verifySignedNodeRelease(KMS.body, KMS.bundle)).rejects.toThrow(
      `The release was signed by "${KMS_IDENTITY}", not by ${NODE_RELEASE_SIGNER.identity}`,
    );
  });

  it('refuses an OIDC issuer other than the one pinned', async () => {
    await expect(
      verifySignedNodeRelease(R87.publishedMrtds, R87.bundle, { issuer: 'https://accounts.google.com' }),
    ).rejects.toThrow(
      `The signing certificate's OIDC issuer is "${NODE_RELEASE_SIGNER.issuer}", not https://accounts.google.com`,
    );
  });

  it('refuses a signed entry timestamp that was altered', async () => {
    const bundle = edited((b) => {
      b.rekorBundle.SignedEntryTimestamp = flipBase64(b.rekorBundle.SignedEntryTimestamp, 20);
    });
    await expect(verifySignedNodeRelease(R87.publishedMrtds, bundle)).rejects.toThrow(
      "Rekor's signed entry timestamp does not verify",
    );
  });

  it('refuses a logged time, or index, that was altered', async () => {
    for (const field of ['integratedTime', 'logIndex']) {
      const bundle = edited((b) => {
        b.rekorBundle.Payload[field] -= 1;
      });
      await expect(verifySignedNodeRelease(R87.publishedMrtds, bundle)).rejects.toThrow(
        "Rekor's signed entry timestamp does not verify",
      );
    }
  });

  it('refuses an entry from a log the trust root does not know', async () => {
    const bundle = edited((b) => {
      b.rekorBundle.Payload.logID = 'ab'.repeat(32);
    });
    await expect(verifySignedNodeRelease(R87.publishedMrtds, bundle)).rejects.toThrow(
      'which the Sigstore trust root does not know',
    );
  });

  it("refuses another release's certificate, or a byte flipped in this one", async () => {
    const swapped = edited((b) => {
      b.cert = JSON.parse(R86.bundle).cert;
    });
    await expect(verifySignedNodeRelease(R87.publishedMrtds, swapped)).rejects.toThrow(
      "The bundle's certificate is not the one Rekor logged",
    );
    const der = leafDer(R87.bundle);
    der[der.length - 10] ^= 0x01; // a byte of the issuer's signature
    const flipped = edited((b) => {
      b.cert = btoa(toPem(der));
    });
    await expect(verifySignedNodeRelease(R87.publishedMrtds, flipped)).rejects.toThrow(
      "The bundle's certificate is not the one Rekor logged",
    );
  });

  it("refuses a signature that is not the logged one, or the detached one's", async () => {
    const other = edited((b) => {
      b.base64Signature = JSON.parse(R86.bundle).base64Signature;
    });
    await expect(verifySignedNodeRelease(R87.publishedMrtds, other)).rejects.toThrow(
      "The bundle's signature is not the one Rekor logged",
    );
    await expect(
      verifySignedNodeRelease(R87.publishedMrtds, R87.bundle, { signature: R86.signature }),
    ).rejects.toThrow('The detached signature is not the one in the bundle');
  });

  it("refuses a file whose tag is not the version expected", async () => {
    await expect(
      verifySignedNodeRelease(R87.publishedMrtds, R87.bundle, { expectedVersion: '2.3.86' }),
    ).rejects.toThrow('The signed release is 2.3.87, not 2.3.86');
  });

  it('refuses a signed file that is not a node release', async () => {
    await expect(verifySignedNodeRelease(KMS.body, KMS.bundle, { identity: KMS_IDENTITY })).rejects.toThrow(
      'The signed release is a "kms" release, not a node one',
    );
    // Signed by the node workflow, with a role and profiles, but no measurements.
    await expect(verifySignedNodeRelease(PROVENANCE.body, PROVENANCE.bundle)).rejects.toThrow(
      "not a release's published-mrtds.json",
    );
  });

  it('refuses what is not a bundle', async () => {
    await expect(verifySignedNodeRelease(R87.publishedMrtds, 'not json')).rejects.toThrow('The bundle is not JSON');
    await expect(verifySignedNodeRelease(R87.publishedMrtds, {})).rejects.toThrow('is not a cosign bundle');
    const bigIndex = edited((b) => {
      b.rekorBundle.Payload.logIndex = 2 ** 60;
    });
    await expect(verifySignedNodeRelease(R87.publishedMrtds, bigIndex)).rejects.toThrow('is not a cosign bundle');
  });
});

describe('the certificate chain', () => {
  const leaf = parseCertificate(leafDer(R87.bundle));
  const logged = JSON.parse(R87.bundle).rekorBundle.Payload.integratedTime * 1000;

  it('chains the real leaf to Fulcio when Rekor logged it, and not ten minutes later', async () => {
    await expect(chainsToFulcio(leaf, logged)).resolves.toBeUndefined();
    await expect(chainsToFulcio(leaf, leaf.notAfter + 1000)).rejects.toThrow(
      'not at 2026-09-28T10:29:25.000Z when Rekor logged it',
    );
  });

  it('refuses a leaf whose issuer signature does not verify', async () => {
    const der = leafDer(R87.bundle);
    der[der.length - 10] ^= 0x01;
    await expect(chainsToFulcio(parseCertificate(der), logged)).rejects.toThrow(
      'was not issued by a Fulcio CA in the Sigstore trust root',
    );
  });

  it("names the CA when its trust-root window does not cover the time", async () => {
    // The intermediate that issued the leaf, used before the trust root says it was.
    await expect(chainsToFulcio({ ...leaf, notBefore: 0 }, Date.UTC(2022, 0, 1))).rejects.toThrow(
      'the trust root says it was not issuing certificates then',
    );
  });

  it('embeds a trust root that is consistent: log ids are their keys, chains verify', async () => {
    for (const log of REKOR_LOGS) {
      const digest = await crypto.subtle.digest('SHA-256', fromBase64(log.publicKey, 'key'));
      expect(hex(new Uint8Array(digest))).toBe(log.logId);
    }
    for (const authority of FULCIO_AUTHORITIES) {
      const chain = authority.certChain.map((der) => parseCertificate(fromBase64(der, 'cert')));
      for (const [index, certificate] of chain.entries()) {
        const parent = chain[index + 1] ?? certificate;
        expect(certificate.curve).toBe('P-384');
        expect(certificate.signatureHash).toBe('SHA-384');
        await expect(
          verifyEcdsa(parent.spki, parent.curve, certificate.signatureHash, certificate.signature, certificate.tbs),
        ).resolves.toBe(true);
      }
    }
  });
});

describe('trustSignedRelease', () => {
  it('trusts one profile of a signed release at or above the minimum', async () => {
    const trusted = await trustSignedRelease({
      release: signed('2.3.87'),
      profile: 'locked-read-only',
      minReleaseVersion: '2.3.86',
    });
    const image = JSON.parse(R87.publishedMrtds).profiles['locked-read-only'];
    expect(trusted.allowedMeasurements).toEqual([
      { mrtd: image.mrtd, rtmr0: image.rtmr0, rtmr1: image.rtmr1, rtmr2: image.rtmr2, rtmr3: image.rtmr3 },
    ]);
    expect(trusted.allowedTcbStatuses).toContain('UpToDate');
    await expect(
      trustSignedRelease({ release: signed('2.3.87'), profile: 'debug', minReleaseVersion: '2.3.87' }),
    ).rejects.toThrow('not a production profile');
    await expect(
      trustSignedRelease({
        release: signed('2.3.87'),
        profile: 'debug',
        minReleaseVersion: '2.3.87',
        allowDebugProfile: true,
      }),
    ).resolves.toBeDefined();
  });

  it('refuses a release older than the minimum, compared as versions and not strings', async () => {
    await expect(
      trustSignedRelease({ release: signed('2.3.86'), profile: 'locked-read-only', minReleaseVersion: '2.3.87' }),
    ).rejects.toThrow('Release 2.3.86 is older than the minimum trusted, 2.3.87');
    await expect(
      trustSignedRelease({ release: signed('2.3.87'), profile: 'locked-read-only', minReleaseVersion: '2.3.100' }),
    ).rejects.toThrow('older than the minimum');
    await expect(
      trustSignedRelease({ release: signed('2.3.87'), profile: 'locked-read-only', minReleaseVersion: '2.3.87-rc.1' }),
    ).resolves.toBeDefined();
    await expect(
      trustSignedRelease({ release: signed('2.3.87'), profile: 'locked-read-only', minReleaseVersion: '2.3.9' }),
    ).resolves.toBeDefined();
  });

  it('requires a minimum that is a version', async () => {
    for (const junk of [undefined, '', 'latest', '2.3', '2.3.x', 'v2.3.87', '2.3.87-', '../2.3.87']) {
      await expect(
        trustSignedRelease({
          release: signed('2.3.87'),
          profile: 'locked-read-only',
          minReleaseVersion: junk as string,
        }),
      ).rejects.toThrow('minReleaseVersion is');
    }
  });

  it("refuses a release that claims a version other than its file's", async () => {
    await expect(
      trustSignedRelease({
        release: { ...signed('2.3.87'), version: '2.3.88' },
        profile: 'locked-read-only',
        minReleaseVersion: '2.3.86',
      }),
    ).rejects.toThrow('The signed release is 2.3.87, not 2.3.88');
    await expect(
      trustSignedRelease({
        release: { ...signed('2.3.87'), version: 'latest' },
        profile: 'locked-read-only',
        minReleaseVersion: '2.3.86',
      }),
    ).rejects.toThrow('The release version is "latest"');
  });

  it("refuses a release whose file is not the one signed", async () => {
    await expect(
      trustSignedRelease({
        release: { ...signed('2.3.87'), publishedMrtds: TAMPERED },
        profile: 'locked-read-only',
        minReleaseVersion: '2.3.86',
      }),
    ).rejects.toThrow("The file's SHA-256 is not the one Rekor logged");
  });

  it('refuses a profile the release does not have', async () => {
    await expect(
      trustSignedRelease({ release: signed('2.3.87'), profile: 'prod', minReleaseVersion: '2.3.86' }),
    ).rejects.toThrow('has no profile "prod"');
  });
});

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });

describe('fetching a release', () => {
  const serving = (body: unknown, status = 200) =>
    vi.fn(async () => new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } }));

  it('unwraps data from a mirror, which is only fetched', async () => {
    const fetch = serving({ data: signed('2.3.87') });
    const url = cloudNodeReleaseUrl('https://cloud.example/', '2.3.87');
    expect(url).toBe('https://cloud.example/api/tee/node-releases/2.3.87');
    await expect(fetchNodeRelease(url, { fetch })).resolves.toEqual(signed('2.3.87'));
    expect(fetch).toHaveBeenCalledWith(url, expect.anything());
    expect(() => cloudNodeReleaseUrl('https://cloud.example', '../admin')).toThrow('not a release version');
    expect(DEFAULT_RELEASE_MIRROR).toBe('https://cloud.calimero.network');
  });

  it('refuses what is not a release', async () => {
    const url = 'https://cloud.example/api/tee/node-releases/2.3.87';
    await expect(fetchNodeRelease(url, { fetch: serving({ data: {} }) })).rejects.toThrow(
      'did not answer with a signed node release',
    );
    await expect(fetchNodeRelease(url, { fetch: serving({}, 404) })).rejects.toThrow('HTTP 404');
  });
});

// The real TDX quote from dcap-qvl's samples, as verify.test.ts uses it. It is
// not a mero-tee image, so a verifier trusting a real release refuses it at
// the measurement check, after everything before that has passed.
const QUOTE = new Uint8Array(readFileSync(fixture('tdx_quote.bin')));
const COLLATERAL = JSON.parse(text('tdx_quote_collateral.json')) as DcapCollateral;
const AT = 1_751_000_000_000;
const sampleTd = dcapVerify(QUOTE, COLLATERAL, AT / 1000).report.data as { reportData: Uint8Array };
const NONCE = hex(sampleTd.reportData.slice(0, 32));
const SUFFIX = hex(sampleTd.reportData.slice(32));
const QUOTE_B64 = Buffer.from(QUOTE).toString('base64');

describe('createSignedReleaseVerifier', () => {
  const args = { quoteB64: QUOTE_B64, nonce: NONCE, reportDataSuffix: SUFFIX, collateral: COLLATERAL };
  const verifier = (release: () => Promise<SignedNodeRelease>, minReleaseVersion = '2.3.86') =>
    createSignedReleaseVerifier({ dcapVerify, release, profile: 'locked-read-only', minReleaseVersion, now: () => AT });

  it('verifies the quote against the signed release, and refuses an image not of it', async () => {
    const release = vi.fn(async () => signed('2.3.87'));
    const verify = verifier(release);
    expect(verify.includeCollateral).toBe(true);
    await expect(verify(args)).rejects.toThrow('are not an image this verifier trusts');
    expect(release).toHaveBeenCalledTimes(1);
  });

  it('refuses before the quote when the release does not pass', async () => {
    await expect(verifier(async () => signed('2.3.86'), '2.3.87')(args)).rejects.toThrow('older than the minimum');
    await expect(verifier(async () => ({ ...signed('2.3.87'), publishedMrtds: TAMPERED }))(args)).rejects.toThrow(
      'SHA-256',
    );
    expect(() => verifier(async () => signed('2.3.87'), 'latest')).toThrow('minReleaseVersion');
  });
});
