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
  for a dedicated disk identity (a protobuf libp2p keypair, created on request).
  `KmsDiskKeyCommand` (`merod kms disk-key`) is the boot-script form: it writes
  the key only to tmpfs/ramfs, never to a device or stdout.
- **Config** (`config.rs`): `TeeConfig` / `KmsConfig` / `KmsAttestationConfig`, as
  merod's `config.toml` held them.
- **mock-attestation** (feature, off by default): accepts mock quotes where a test
  config allows them. Never in a shipped build.

## Commands

```bash
cargo test -p calimero-kms-client --features mock-attestation
cargo clippy -p calimero-kms-client --all-targets --features mock-attestation -- -D warnings
```
