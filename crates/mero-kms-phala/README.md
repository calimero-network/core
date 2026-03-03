# mero-kms-phala

Key release service for `merod` nodes running in a TEE.

`mero-kms-phala` validates node attestations and only releases storage keys when
the request satisfies both:

1. identity/freshness checks (challenge-response + peer signature), and
2. measurement policy checks (TCB status + MRTD/RTMR allowlists).

## Endpoints

### `POST /challenge`

Issue a short-lived, single-use challenge nonce.

Request:

```json
{
  "peerId": "12D3KooW..."
}
```

Response:

```json
{
  "challengeId": "a1b2c3d4...",
  "nonceB64": "base64-32-byte-nonce",
  "expiresAt": 1735689600
}
```

### `POST /get-key`

Verify the attestation and release a deterministic key from dstack KMS.

Request:

```json
{
  "challengeId": "a1b2c3d4...",
  "quoteB64": "...",
  "peerId": "12D3KooW...",
  "peerPublicKeyB64": "...",
  "signatureB64": "..."
}
```

The service verifies:

- challenge exists, is not expired, and is consumed once,
- `peerPublicKey` maps to claimed `peerId`,
- `signature` is valid for the signed payload,
- quote is cryptographically valid,
- quote report data contains:
  - challenge nonce in bytes `[0..32]`,
  - `sha256(peer_id)` in bytes `[32..64]`,
- quote measurements/TCB satisfy configured policy.

## Configuration

Environment variables:

- `LISTEN_ADDR` (default: `0.0.0.0:8080`)
- `DSTACK_SOCKET_PATH` (default: `/var/run/dstack.sock`)
- `CHALLENGE_TTL_SECS` (default: `60`)
- `ACCEPT_MOCK_ATTESTATION` (default: `false`)
- `ENFORCE_MEASUREMENT_POLICY` (default: `true`)
- `ALLOWED_TCB_STATUSES` (CSV, default: `UpToDate`)
- `ALLOWED_MRTD` (CSV of hex measurements)
- `ALLOWED_RTMR0` (CSV of hex measurements)
- `ALLOWED_RTMR1` (CSV of hex measurements)
- `ALLOWED_RTMR2` (CSV of hex measurements)
- `ALLOWED_RTMR3` (CSV of hex measurements)

Measurement values must be hex-encoded 48-byte values (96 hex chars, optional
`0x` prefix).

When strict policy is enabled (`ENFORCE_MEASUREMENT_POLICY=true`) and mock
attestation is disabled (`ACCEPT_MOCK_ATTESTATION=false`):

- `ALLOWED_MRTD` must contain at least one trusted value.
- `ALLOWED_TCB_STATUSES` must not be empty.

## Production guidance

- Keep `ACCEPT_MOCK_ATTESTATION=false`.
- Pin trusted values from your built/deployed image:
  - MRTD (required),
  - RTMR0/1/2 (boot/runtime chain),
  - RTMR3 (application/compose/runtime extensions).
- Start with `ALLOWED_TCB_STATUSES=UpToDate`.
- Use a short challenge TTL (for example, `30-120` seconds).

Example:

```bash
export LISTEN_ADDR=0.0.0.0:8080
export DSTACK_SOCKET_PATH=/var/run/dstack.sock
export CHALLENGE_TTL_SECS=60
export ACCEPT_MOCK_ATTESTATION=false
export ENFORCE_MEASUREMENT_POLICY=true
export ALLOWED_TCB_STATUSES=UpToDate
export ALLOWED_MRTD=<trusted_mrtd_hex>
export ALLOWED_RTMR0=<trusted_rtmr0_hex>
export ALLOWED_RTMR1=<trusted_rtmr1_hex>
export ALLOWED_RTMR2=<trusted_rtmr2_hex>
export ALLOWED_RTMR3=<trusted_rtmr3_hex>
```

## Development mode

For local testing without real TDX hardware, you can set:

```bash
export ACCEPT_MOCK_ATTESTATION=true
```

Do not use mock attestation in production.

## Deployment

For a complete guide on building images, deploying to Phala Cloud, and configuring merod for TEE nodes, see [Deploy merod on Phala Network (TEE)](../../docs/phala-tee-deployment.md).
