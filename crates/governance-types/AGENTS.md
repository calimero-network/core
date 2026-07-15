# calimero-governance-types - Signed Group-Governance Op Types

Pure-data types for local (no-chain) group/namespace governance: signed operations, their signing/verification, borsh wire layout, and gossip envelopes.

## Package Identity

- **Crate**: `calimero-governance-types`
- **Entry**: `src/lib.rs` (+ `src/wire.rs` submodule, `src/tests.rs`)
- **Key deps**: `borsh` (wire format), `ed25519-dalek` (signature errors), `sha2` (op content hash), `blake3` (topic-scoped ack hash), `thiserror`, `calimero-context-config` (`AppKey`, `ContextGroupId`, `SignedGroupOpenInvitation`, `MemberCapabilities`, `VisibilityMode`), `calimero-primitives` (`PublicKey`/`PrivateKey`, `ContextId`, `GroupMemberRole`, `UpgradePolicy`, `ApplicationId`, `BlobId`), `calimero-storage` (`HybridTimestamp` for the HLC cascade fence)
- **Deliberately no `actix` dependency**: this crate is consumed by both `calimero-context-client` (actix runtime) and `calimero-governance-store`, which must not transitively re-acquire actix through the op-type dependency (#2479, epic #2300)

## Commands

```bash
# Build
cargo build -p calimero-governance-types

# Test (all, including golden-byte and round-trip suites)
cargo test -p calimero-governance-types

# Test a single case
cargo test -p calimero-governance-types sign_and_verify_round_trip -- --nocapture
```

## Type Inventory

| Item | Kind | Purpose |
| --- | --- | --- |
| `GroupOp` | enum, `#[non_exhaustive]` | Mutation of a single group's state (membership, roles, capabilities, metadata, TEE policy, cascades, key rotation) |
| `SignableGroupOp` / `SignedGroupOp` | struct | Unsigned/signed envelope around a `GroupOp`: `version`, `group_id`, `parent_op_hashes`, `signer`, `nonce`, `op`, (+`signature`) |
| `RootOp` | enum (NOT `#[non_exhaustive]`) | Cleartext namespace-wide op: group create/reparent/delete, admin change, policy update, member join, key delivery, namespace genesis |
| `NamespaceOp` | enum, `#[non_exhaustive]` | `Root(RootOp)` or `Group { group_id, key_id, encrypted: EncryptedGroupOp, key_rotation }` - the encrypted variant is how a `GroupOp` actually travels on the namespace DAG |
| `SignableNamespaceOp` / `SignedNamespaceOp` | struct | Same shape as the group envelope but wraps `NamespaceOp` and is scoped by `namespace_id` |
| `EncryptedGroupOp` | struct | `{ nonce: [u8;12], ciphertext: Vec<u8> }` - AES-256-GCM(borsh(GroupOp)) under the group key |
| `KeyEnvelope` | struct | ECDH-wrapped group key for one recipient: `recipient`, `sender`, `ephemeral_pk`, `nonce`, `ciphertext`, `signature` |
| `KeyRotation` | struct | `{ new_key_id, envelopes: Vec<KeyEnvelope> }` attached to `MemberRemoved`; one envelope per remaining member, none for the removed member |
| `OpaqueSkeleton` | struct | What a non-member stores for a `Group`-scoped op it cannot decrypt: `delta_id`, `parent_op_hashes`, `group_id`, `signer` |
| `StoredNamespaceEntry` | enum | Tagged storage row: `Signed(SignedNamespaceOp)` or `Opaque(OpaqueSkeleton)` |
| `NamespaceId`, `KeyId` | newtype (`id_newtype!` macro) | 32-byte ids, borsh- and serde-transparent (serialize as the bare `[u8;32]`) |
| `ContextCapabilityBits` | struct | Non-zero `u8` capability bitmask; zero rejected at construction AND at borsh deserialize |
| `GovernanceError` | enum (`thiserror`) | `SchemaVersion`, `Signature` (from `SignatureError`), `BorshSerialize` (from `io::Error`), `Bounds` |
| `bounds` (module) | consts | Anti-amplification caps for decoded-op field lengths (see Invariants) |
| `wire::NamespaceTopicMsg` / `wire::GroupTopicMsg` | enum | Discriminated gossipsub envelope: `Op`, `Ack`, `ReadinessBeacon`, `ReadinessProbe`, (+`MigrationHeartbeat` on the namespace topic) |
| `wire::SignedAck`, `wire::SignedReadinessBeacon`, `wire::SignedMigrationHeartbeat`, `wire::ReadinessProbe` | struct | Signed gossip primitives layered on top of ops (see Mental Model) |

## Mental Model

A **namespace** has one governance DAG. Every delta carries one `NamespaceOp`: `Root` ops are cleartext and visible to every namespace member; `Group` ops carry a cleartext `group_id`/`key_id` for routing but the actual `GroupOp` payload is `EncryptedGroupOp` - AES-256-GCM(borsh(GroupOp)) under the group's symmetric key. A non-member cannot decrypt a `Group` op, so it stores an `OpaqueSkeleton` instead - just enough (`delta_id`, `parent_op_hashes`, `group_id`, `signer`) to keep the DAG's causal structure intact without seeing the mutation.

Signing is always the same three-step shape, duplicated for the group envelope and the namespace envelope:

1. Build a `Signable*Op` (the envelope minus `signature`).
2. `*_signable_bytes()` = a domain-separation prefix (`GROUP_GOVERNANCE_SIGN_DOMAIN` / `NAMESPACE_GOVERNANCE_SIGN_DOMAIN`) prepended to `borsh::to_vec(signable)`. The domain prefix stops a signature from one protocol surface being replayed as a different one.
3. `Signed*Op::sign(sk, ...)` signs those bytes with Ed25519; `verify_signature()` recomputes them from `self` and checks the signature. `content_hash()` is `SHA-256(signable_bytes)` - the op's stable id for DAG parent links and idempotency/dedup.

`verify_signature()` proves cryptographic integrity only: it does not check that `signer` is an authorized member/admin. That authorization check happens one layer up, in the apply path (`calimero-context` / `calimero-governance-store`) via `membership_status_at` / `is_group_admin` / `is_authoritative_namespace_identity`.

DAG ordering is carried by `parent_op_hashes` (content hashes of the DAG heads at signing time - empty for a group's genesis op, multiple entries after a concurrent merge), not by any field inside the op itself.

Key rotation (forward secrecy) rides as a sidecar: `MemberRemoved` triggers a `KeyRotation` minted by the admin who publishes it (the admin stays in the group so can mint the new key). `MemberLeft` cannot carry one - the leaver would have to mint and thus retain the very key they're being cut off from - so it just records the debt (`PendingRotationRepository`, in `calimero-governance-store`), and a later `GroupKeyRotated` op published by a remaining admin discharges it. `GroupKeyRotated` mutates no state of its own; applying it twice is a harmless no-op.

The gossip layer (`wire.rs`) is a separate concern layered on top of ops: `NamespaceTopicMsg`/`GroupTopicMsg` are what's actually published on `ns/<id>`/`group/<id>` topics, wrapping either an `Op` or one of the non-op signed primitives (`Ack`, `ReadinessBeacon`, `ReadinessProbe`, `MigrationHeartbeat`). Each of those has its own domain-separated signable-bytes + verify pair, exactly mirroring the op-signing pattern. `hash_scoped_namespace`/`hash_scoped_group` (`blake3(topic_id || borsh(op))`) bind an ack to the topic it was published on, so an ack for one namespace can't be replayed against an identical op on a different one.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | `GroupOp`, `RootOp`, `NamespaceOp`, signed/signable envelopes, `KeyEnvelope`/`KeyRotation`, `GovernanceError`, `bounds`, all `validate()` impls |
| `src/wire.rs` | `NamespaceTopicMsg`/`GroupTopicMsg`, `SignedAck`, `SignedReadinessBeacon`, `SignedMigrationHeartbeat`, `ReadinessProbe`, plus its own `#[cfg(test)]` module |
| `src/tests.rs` | Sign/verify round trips, tampering-must-fail tests, and the golden frozen-byte discriminant tests for `GroupOp`/`RootOp` |

## Invariants and Gotchas

- **Borsh discriminants are append-only, positional, and permanent.** Every enum variant's wire tag is its source order. New `GroupOp`/`RootOp`/`NamespaceOp` variants MUST be appended at the end - inserting one in the middle silently renumbers every later variant. `RootOp` is deliberately NOT `#[non_exhaustive]` so the apply-side exhaustive `match` fails to compile on a missing handler; `GroupOp`/`NamespaceOp` ARE `#[non_exhaustive]` for other reasons but the append-only rule still applies. `src/tests.rs` guards this with **golden frozen-byte** tests: each test decodes a hand-written byte vector with the CURRENT enum, never re-encodes it, because an encode-then-decode round trip in the same binary cannot catch a mid-enum insertion (both sides agree on the same shifted ordinal).
- **`SIGNED_GROUP_OP_SCHEMA_VERSION` (currently 8) and `SIGNED_NAMESPACE_OP_SCHEMA_VERSION` (currently 3) are checked strictly-equal on `verify_signature()`.** Bumping either is a flag-day: every op's content hash changes and old-shape ops are rejected outright, not migrated. Read the version-history doc comments above each constant before touching either enum's field layout - some past changes (e.g. `GroupKeyRotated`, the v6/v7 cascade variants) deliberately did NOT bump the version because doing so would make every older peer reject every op, not just the new variant; append-and-don't-bump was the safer rollout.
- **Two DEPRECATED cascade variants exist on purpose.** `CascadeTargetApplicationSet` and `CascadeGroupMigrationSet` are superseded by the atomic `CascadeUpgrade` (which fixes an out-of-order-apply bug) but their apply arms are retained for one release so in-flight/replayed ops from pre-upgrade peers still apply. They are not marked `#[deprecated]` because the enum's derived borsh/Debug impls reference every variant and would warn at the derive site - do not remove the apply handling without checking the rollout window has passed.
- **`ContextCapabilityBits` cannot be zero, enforced twice**: `new()`/`try_from` reject it at construction, and a hand-written `BorshDeserialize` impl rejects a zero byte on the wire too - so a zero-capability op cannot even be constructed by decoding untrusted bytes. Unknown non-zero bits are accepted (forward-compatible replay by older nodes); callers must mask against known constants before interpreting.
- **`bounds` module and `validate()` exist purely against a malicious/buggy peer.** Legitimate ops are already capped by `MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES` (64 KiB) upstream on the send path; borsh itself has no inherent per-element cap when decoding gossip/backfill bytes, so `SignedGroupOp::validate()`/`SignedNamespaceOp::validate()` reject egregiously oversized decoded ops (parent list, ciphertext, key-envelope count, id lists, TEE allow-list entries, metadata maps) before they reach apply/storage. Call `validate()` right after decode, before doing anything else with an untrusted op.
- **`SignedMigrationHeartbeat` has a hand-rolled `BorshDeserialize`, not the derive.** `authored_remaining` (u64) and `migration_failed` (u8) are trailing fields appended AFTER the signed body, read via `read_trailing` with EOF-tolerance so an old-format heartbeat missing them decodes to `0` rather than erroring. Both fields are deliberately OUTSIDE the signed payload (advisory telemetry, not a migration gate) - tampering them never breaks `verify_signature()`. Adding a new field: append after `migration_failed` (another trailing read) or introduce a version discriminant; do NOT reorder the existing prefix fields, and keep the round-trip + mixed-fleet tests in `wire.rs` in step.
- **`verify_signature()` is integrity, not authorization.** Every `verify_signature()` doc comment says this explicitly - treating a passing verify as "this signer is allowed to do this" is a real vulnerability class (accepting ops from revoked/never-admitted keys). Authorization is the caller's job.
- **`RootOp::NamespaceCreated` is the self-authorizing namespace genesis** (#2474): it is the only op whose apply does NOT call `require_namespace_admin`, because it's what establishes admin authority in the first place. Its anti-hijack property (apply is a no-op if root meta already exists) only protects an already-established namespace - the very first genesis on a bare replica is trust-on-first-sync (see #2932 in the doc comment).
- Domain separation prefixes (`GROUP_GOVERNANCE_SIGN_DOMAIN`, `NAMESPACE_GOVERNANCE_SIGN_DOMAIN`, `KEY_ENVELOPE_SIGN_DOMAIN`, `wire::ACK_SIGN_DOMAIN`, `wire::READINESS_BEACON_SIGN_DOMAIN`, `wire::MIGRATION_HEARTBEAT_SIGN_DOMAIN`) are each distinct so a signature can never be lifted from one protocol surface (e.g. an ack) and replayed as another (e.g. an op). Never reuse a domain string across two signable-bytes functions.

## Relation to calimero-governance-store

This crate is pure data: types, signing, hashing, borsh layout, and validation bounds - no apply logic, no storage, no actix. `calimero-governance-store` (and `calimero-context`) is the consumer that:

- decodes `SignedGroupOp`/`SignedNamespaceOp` off the wire, calls `.validate()` then `.verify_signature()`,
- performs the actual authorization checks these types deliberately don't (`membership_status_at`, `is_group_admin`, per-op permission checks),
- applies each `GroupOp`/`RootOp` variant to durable state, dispatching on the type's own `op_kind_label()` for metrics,
- owns `PendingRotationRepository` (the ledger of rotations owed after a `MemberLeft`, discharged by a later `GroupKeyRotated`),
- and drives `AckRouter`/`ReadinessManager`/`MigrationStatusCache`, the actix-dependent consumers of `wire::SignedAck` / `wire::SignedReadinessBeacon` / `wire::SignedMigrationHeartbeat`.

`crates/context`, `crates/context/primitives`, and `crates/op-adapter` also depend on this crate directly for the op types without pulling in the store's apply machinery.

Part of [crates/](../AGENTS.md).
