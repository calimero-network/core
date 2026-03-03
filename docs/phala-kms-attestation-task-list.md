# Phala KMS Attestation Migration Tasks (merod)

Status: In progress
Owner: merod team
Last updated: 2026-03-03

This checklist tracks the migration to mandatory KMS self-attestation verification
for merod startup key retrieval.

## Phase 1: merod-side protocol changes

- [x] Add KMS `/attest` preflight call before `/challenge`.
- [x] Verify quote freshness with nonce and binding (`reportData[0..32]`, `reportData[32..64]`).
- [x] Add KMS TCB/measurement allowlist checks in merod.
- [x] Add config fields for KMS attestation policy (`tee.kms.phala.attestation.*`).
- [x] Add startup validation that attestation policy is not empty when enabled.
- [ ] Add integration test coverage for successful `/attest` + `/challenge` + `/get-key` sequence.
- [ ] Add integration test coverage for rejected KMS measurements.

## Phase 2: operational hardening

- [ ] Turn on `tee.kms.phala.attestation.enabled=true` in production defaults.
- [ ] Publish trusted KMS measurement values in release/governance artifacts.
- [ ] Ensure load-balancer/session behavior keeps `/challenge` and `/get-key` on compatible challenge state.
- [ ] Add blue/green rollout playbook:
  - old merod release talks only to old KMS deployment,
  - new merod release talks only to new KMS deployment,
  - no cross-version KMS sharing between deployments.
- [ ] Require per-deployment policy pinning to a specific signed `mero-tee` release tag (never `latest`).

## Phase 3: policy/governance integration

- [ ] Source KMS measurement allowlists from signed/governed artifact rather than mutable config only.
- [ ] Add audit logging for KMS attestation decisions (measurement fingerprints, policy version).
- [ ] Define emergency revoke procedure for compromised KMS measurement.

## Notes

- Attestation verification must fail closed in production.
- Mock quote acceptance (`attestation.accept_mock=true`) is for development only.
- Only TEE nodes (with `[tee]` configured) use KMS; non-TEE nodes are libp2p-only and do not call KMS.
