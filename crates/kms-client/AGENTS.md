# calimero-kms-client

The mero-kms client a TEE node fetches its keys with. It was merod's (`src/kms`,
`kms_policy.rs`, `kms disk-key`); merod uses it in process.

- **Release policy** (`policy.rs`): the KMS is trusted only if its `/attest` quote
  matches the signed mero-tee release the environment names (`MERO_TEE_VERSION`,
  `MERO_KMS_VERSION`, `MERO_KMS_RELEASE_TAG`), fetched and Sigstore-verified with
  `calimero-tee-release`. No release named: no key.
- **Key release** (`kms/`): challenge, this TD's quote over it, a signed request with
  the identity's libp2p key; the KMS answers with the key for
  `{namespace}/{profile}/{peerId}`, sealed to the request (`kms/sealed.rs`).
- **Disk key** (`disk_key.rs`): `fetch_disk_key(url, identity, create)` returns the key
  for a dedicated disk identity (a protobuf libp2p keypair, created on request);
  `identity_peer_id(path)` reads one's peer id without creating it (`merod attest`).
  `KmsDiskKeyCommand` (`merod kms disk-key`) is the boot-script form: it writes
  the key only to tmpfs/ramfs, never to a device or stdout.
- **KMS evidence**: the release-policy check returns, with the KMS's transport
  key, a `KmsEvidence` (`calimero-tee-attestation`) of the quote that passed:
  registers, TCB status, the signed policy's `release`/`profile`, the KMS host,
  the quote's SHA-256 and the time. `fetch_storage_key` answers a `ReleasedKey
  { key, evidence }` (no `Debug`: it holds the key); `fetch_disk_key` answers
  the key and its evidence. `kms disk-key --evidence-out` writes it (tmpfs/ramfs
  only); `read_evidence` reads it back, refusing any other filesystem, more
  than `MAX_EVIDENCE_BYTES` or an unknown field. The node reports it, bound
  into its quote, at `GET /attestation?purpose=kms`.
- **Config** (`config.rs`): `TeeConfig` / `KmsConfig` / `KmsAttestationConfig`, as
  merod's `config.toml` held them.
- **mock-attestation** (feature, off by default): accepts mock quotes where a test
  config allows them. Never in a shipped build.

## Commands

```bash
cargo test -p calimero-kms-client --features mock-attestation
cargo clippy -p calimero-kms-client --all-targets --features mock-attestation -- -D warnings
```
