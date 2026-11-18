/**
 * The parts of the Sigstore public-good trust root that verifying a mero-tee
 * release's cosign bundle needs: the Fulcio certificate authorities that issue
 * the release workflow's short-lived signing certificates, and the Rekor log
 * key that signs the bundle's signed entry timestamp. Each with the window in
 * which the trust root says it was in use.
 *
 * Copied from the `trusted_root.json` that Sigstore publishes through its TUF
 * repository (https://tuf-repo-cdn.sigstore.dev, target `trusted_root.json`),
 * as shipped by the `sigstore` Python client 4.5.0 in
 * `sigstore/_store/https%3A%2F%2Ftuf-repo-cdn.sigstore.dev/trusted_root.json`.
 * Embedded rather than fetched: fetching it at run time would trust whoever
 * serves it, the very thing this check exists to avoid.
 *
 * Left out, because a legacy cosign bundle never uses them: the CT logs (the
 * certificate's SCT is not checked here, as core does not either), the
 * timestamp authority, and the Rekor v2 log `log2025-1.rekor.sigstore.dev`,
 * whose Ed25519 key signs checkpoints rather than signed entry timestamps. A
 * bundle that names any log but those below is refused.
 *
 * To refresh it when Sigstore rotates a key or adds a CA: take the current
 * `trusted_root.json` from a TUF client (`python -m sigstore` or `cosign
 * initialize` both keep one, verified against the TUF root), and copy each
 * `certificateAuthorities[].certChain.certificates[].rawBytes` with its
 * `validFor`, and each ECDSA `tlogs[]` entry's `publicKey.rawBytes`,
 * `publicKey.validFor` and `logId.keyId` (as hex). The tests check each log id
 * is the SHA-256 of its key and each chain verifies.
 */

/** A Fulcio CA: its chain from the certificate that issues leaves up to the root, DER as base64. */
export interface FulcioAuthority {
  uri: string;
  /** When the trust root says this CA issued certificates. No `end`: it still does. */
  validFor: { start: string; end?: string };
  certChain: string[];
}

/** A Rekor log whose signed entry timestamps are accepted. */
export interface RekorLog {
  baseUrl: string;
  /** The log id a bundle names it by: the SHA-256 of its public key, hex. */
  logId: string;
  /** Its public key: DER SubjectPublicKeyInfo, base64. ECDSA P-256 with SHA-256. */
  publicKey: string;
  validFor: { start: string; end?: string };
}

export const FULCIO_AUTHORITIES: readonly FulcioAuthority[] = [
  {
    uri: 'https://fulcio.sigstore.dev',
    validFor: { start: '2021-03-07T03:20:29Z', end: '2022-12-31T23:59:59.999Z' },
    certChain: [
      'MIIB+DCCAX6gAwIBAgITNVkDZoCiofPDsy7dfm6geLbuhzAKBggqhkjOPQQDAzAqMRUwEwYDVQQKEwxzaWdzdG9y' +
        'ZS5kZXYxETAPBgNVBAMTCHNpZ3N0b3JlMB4XDTIxMDMwNzAzMjAyOVoXDTMxMDIyMzAzMjAyOVowKjEVMBMGA1UE' +
        'ChMMc2lnc3RvcmUuZGV2MREwDwYDVQQDEwhzaWdzdG9yZTB2MBAGByqGSM49AgEGBSuBBAAiA2IABLSyA7Ii5k+p' +
        'NO8ZEWY0ylemWDowOkNa3kL+GZE5Z5GWehL9/A9bRNA3RbrsZ5i0JcastaRL7Sp5fp/jD5dxqc/UdTVnlvS16an+' +
        '2Yfswe/QuLolRUCrcOE2+2iA5+tzd6NmMGQwDgYDVR0PAQH/BAQDAgEGMBIGA1UdEwEB/wQIMAYBAf8CAQEwHQYD' +
        'VR0OBBYEFMjFHQBBmiQpMlEk6w2uSu1KBtPsMB8GA1UdIwQYMBaAFMjFHQBBmiQpMlEk6w2uSu1KBtPsMAoGCCqG' +
        'SM49BAMDA2gAMGUCMH8liWJfMui6vXXBhjDgY4MwslmN/TJxVe/83WrFomwmNf056y1X48F9c4m3a3ozXAIxAKjR' +
        'ay5/aj/jsKKGIkmQatjI8uupHr/+CxFvaJWmpYqNkLDGRU+9orzh5hI2RrcuaQ==',
    ],
  },
  {
    uri: 'https://fulcio.sigstore.dev',
    validFor: { start: '2022-04-13T20:06:15Z' },
    certChain: [
      'MIICGjCCAaGgAwIBAgIUALnViVfnU0brJasmRkHrn/UnfaQwCgYIKoZIzj0EAwMwKjEVMBMGA1UEChMMc2lnc3Rv' +
        'cmUuZGV2MREwDwYDVQQDEwhzaWdzdG9yZTAeFw0yMjA0MTMyMDA2MTVaFw0zMTEwMDUxMzU2NThaMDcxFTATBgNV' +
        'BAoTDHNpZ3N0b3JlLmRldjEeMBwGA1UEAxMVc2lnc3RvcmUtaW50ZXJtZWRpYXRlMHYwEAYHKoZIzj0CAQYFK4EE' +
        'ACIDYgAE8RVS/ysH+NOvuDZyPIZtilgUF9NlarYpAd9HP1vBBH1U5CV77LSS7s0ZiH4nE7Hv7ptS6LvvR/STk798' +
        'LVgMzLlJ4HeIfF3tHSaexLcYpSASr1kS0N/RgBJz/9jWCiXno3sweTAOBgNVHQ8BAf8EBAMCAQYwEwYDVR0lBAww' +
        'CgYIKwYBBQUHAwMwEgYDVR0TAQH/BAgwBgEB/wIBADAdBgNVHQ4EFgQU39Ppz1YkEZb5qNjpKFWixi4YZD8wHwYD' +
        'VR0jBBgwFoAUWMAeX5FFpWapesyQoZMi0CrFxfowCgYIKoZIzj0EAwMDZwAwZAIwPCsQK4DYiZYDPIaDi5HFKnfx' +
        'Xx6ASSVmERfsynYBiX2X6SJRnZU84/9DZdnFvvxmAjBOt6QpBlc4J/0DxvkTCqpclvziL6BCCPnjdlIB3Pu3BxsP' +
        'mygUY7Ii2zbdCdliiow=',
      'MIIB9zCCAXygAwIBAgIUALZNAPFdxHPwjeDloDwyYChAO/4wCgYIKoZIzj0EAwMwKjEVMBMGA1UEChMMc2lnc3Rv' +
        'cmUuZGV2MREwDwYDVQQDEwhzaWdzdG9yZTAeFw0yMTEwMDcxMzU2NTlaFw0zMTEwMDUxMzU2NThaMCoxFTATBgNV' +
        'BAoTDHNpZ3N0b3JlLmRldjERMA8GA1UEAxMIc2lnc3RvcmUwdjAQBgcqhkjOPQIBBgUrgQQAIgNiAAT7XeFT4rb3' +
        'PQGwS4IajtLk3/OlnpgangaBclYpsYBr5i+4ynB07ceb3LP0OIOZdxexX69c5iVuyJRQ+Hz05yi+UF3uBWAlHpiS' +
        '5sh0+H2GHE7SXrk1EC5m1Tr19L9gg92jYzBhMA4GA1UdDwEB/wQEAwIBBjAPBgNVHRMBAf8EBTADAQH/MB0GA1Ud' +
        'DgQWBBRYwB5fkUWlZql6zJChkyLQKsXF+jAfBgNVHSMEGDAWgBRYwB5fkUWlZql6zJChkyLQKsXF+jAKBggqhkjO' +
        'PQQDAwNpADBmAjEAj1nHeXZp+13NWBNa+EDsDP8G1WWg1tCMWP/WHPqpaVo0jhsweNFZgSs0eE7wYI4qAjEA2WB9' +
        'ot98sIkoF3vZYdd3/VtWB5b9TNMea7Ix/stJ5TfcLLeABLE4BNJOsQ4vnBHJ',
    ],
  },
];

export const REKOR_LOGS: readonly RekorLog[] = [
  {
    baseUrl: 'https://rekor.sigstore.dev',
    logId: 'c0d23d6ad406973f9559f3ba2d1ca01f84147d8ffc5b8445c224f98b9591801d',
    publicKey:
      'MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE2G2Y+2tabdTV5BcGiBIx0a9fAFwrkBbmLSGtks4L3qX6yYY0zufB' +
      'nhC8Ur/iy55GhWP/9A/bY2LhC30M9+RYtw==',
    validFor: { start: '2021-01-12T11:53:27Z' },
  },
];
