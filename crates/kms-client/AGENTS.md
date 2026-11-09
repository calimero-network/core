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
  { key, evidence, counters }` (no `Debug`: it holds the key); `fetch_disk_key` answers
  the key and its evidence. `kms disk-key --evidence-out` writes it (tmpfs/ramfs
  only); `read_evidence` reads it back, refusing any other filesystem, more
  than `MAX_EVIDENCE_BYTES` or an unknown field. The node reports it, bound
  into its quote, at `GET /attestation?purpose=kms`.
- **Rollback counters** (`kms/rollback.rs`, `docs/specs/rollback-protection.md`):
  `/attest` asks for `capabilities`; a mero-kms that keeps counters answers
  `["rollback/v1"]` and commits to them (`attest_binding_v2`), so a relay cannot strip
  them. `fetch_storage_key(.., rollback_node)` then sends `rollbackNodeId` (quote:
  `request_binding_v2`; signed payload) and opens a v2 record (`sealed-key/v2`): the
  key, the cluster epoch, the node's entry and a session. `ReleasedKey::counters`
  (`Counters`) raises (`/rollback/raise`) and resets (`/rollback/reset`) under the
  session key; each answer is HMACed over the node's nonce. An older mero-kms gives
  `counters: None`. Vectors shared with mero-kms: `the_v2_wire_format_matches_the_published_vectors`,
  `the_rollback_macs_match_the_published_vectors`. `disk_key::fetch_disk_key_and_counters`
  is what merod calls.
- **Config** (`config.rs`): `TeeConfig` / `KmsConfig` / `KmsAttestationConfig`, as
  merod's `config.toml` held them.
- **mock-attestation** (feature, off by default): accepts mock quotes where a test
  config allows them. Never in a shipped build.

## Commands

```bash
cargo test -p calimero-kms-client --features mock-attestation
cargo clippy -p calimero-kms-client --all-targets --features mock-attestation -- -D warnings
```
