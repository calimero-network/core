# calimero-account - Account Identity Primitive

The two stable ids that let one person hold several devices: `AccountId` (who) and `DeviceId` (which replica), plus the self-certifying credentials that bind them.

## Package Identity

- **Crate**: `calimero-account`
- **Entry**: `src/lib.rs` (single file, about half of it tests)
- **Key deps**: `borsh` (wire format), `sha2` (content addressing), `thiserror`, `calimero-primitives` (`PublicKey`, with the `borsh` feature)

## Commands

```bash
cargo build -p calimero-account
cargo test -p calimero-account
cargo test -p calimero-account signing_domains_are_pairwise_distinct -- --nocapture
```

## Mental Model: ids are not keys

`executor_id` used to do four jobs at once - signing key, ACL subject, scope-key recipient, and CRDT replica id. One key doing all four is why a person could not hold two devices: sharing it corrupts the CRDT planes (counter slots and HLC seeds assume one writer per replica id), and *not* sharing it splits one person into two unrelated members.

This crate splits the identity half in two:

| | `AccountId` | `DeviceId` |
| --- | --- | --- |
| what it is | a person or agent | one installation |
| authorizes | yes - the only authz subject | never |
| signs | no (only certificates) | yes, every op |
| CRDT replica id | never | yes |
| KEM recipient | no | yes |

**Neither is a public key.** Both are content addresses, so every key can rotate without the identity changing. Making `AccountId` the root public key - the obvious shortcut - would mean the root key can never rotate (rotating = becoming a different person, losing all membership and authorship) and would force it out of cold storage for routine work.

**Self-certifying anchor.** `AccountId` is the content address of the account's `AccountGenesis`, and the genesis names the epoch-0 root key. So the initial root key is recoverable *from the id itself*: given a `DeviceCert`, a verifier checks `genesis.account_id() == cert.account`, then walks a signed `RootKeyHandoff` chain to the cert's epoch - with no prior state and no ordering dependency. That is what lets a freshly paired device link itself into a scope in one self-contained op, instead of the account root having to author a grant into every scope the account belongs to.

**This crate decides nothing.** It verifies self-contained credentials only. The at-cut checks that make a credential *authoritative* - has this root key been superseded, has this device been revoked, is the account even a member here - belong to `calimero-projection` and `calimero-authz`, because only those see the causal cut.

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `AccountId` | struct (`[u8; 32]`) | Content address of an `AccountGenesis`; the only authorization subject |
| `DeviceId` | struct (`[u8; 32]`) | One installation; the CRDT replica id |
| `DeviceId::mint(account, nonce)` | fn | Mint a device id once per installation |
| `DeviceId::hlc_seed()` | fn | First 16 bytes - the HLC instance seed for this replica |
| `KemPublicKey` | struct (`[u8; 32]`) | X25519 scope-key delivery recipient; a distinct type from `PublicKey` |
| `AccountGenesis` | struct | `{version, root_sign_pk, nonce}`; hashing it yields the `AccountId` |
| `AccountGenesis::account_id()` | fn | The id this genesis addresses |
| `RootKeyHandoff` | struct | Rolls the root key from `from_epoch` to `from_epoch + 1`, signed by the outgoing key |
| `DeviceCert` | struct | Root-signed grant binding a device to an account |
| `AccountMemberEndorsement` | struct | A granted member key's signed statement that an account is theirs |
| `sign_account_endorsement` / `verify_account_endorsement` | fn | Mint / check an endorsement; verification says nothing about whether the endorser IS a member |
| `derive_account_nonce(root_secret, namespace_id)` | fn | Per-namespace genesis nonce from the node's account root |
| `VerifiedDeviceCert` | struct | A cert whose anchor, chain and signature all checked - **not** a statement that the binding is in force |
| `resolve_root_keys(genesis, chain)` | fn | Walk the chain; index `i` is the root key at epoch `i` |
| `verify_device_cert(claimed, genesis, chain, cert)` | fn | Full credential check against a claimed account |
| `AccountError` | enum | Why a credential failed |
| `ACCOUNT_GENESIS_VERSION` | const | Version written into a genesis; part of the id preimage |

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Everything: ids, credentials, chain walking, verification, and all tests |

## Invariants and Gotchas

- **`VerifiedDeviceCert` does not mean "in force".** It means the credential is internally valid. Supersession, revocation and scope membership are at-cut questions answered by `calimero-authz`. The type exists so the two stages cannot be confused.
- **Signing domains must stay pairwise distinct.** Every content address and signing preimage goes through `domain_hash`, which **length-prefixes** the domain - a bare concatenation would let a shorter domain that prefixes a longer one collide by shifting bytes across the boundary. `signing_domains_are_pairwise_distinct` and `domain_hash_is_not_confusable_by_shifting_bytes` guard this.
- **`resolve_root_keys` caps the chain at `MAX_ROOT_KEY_HANDOFFS` (1024) before verifying anything.** Each entry costs an Ed25519 verification and the function is reachable from untrusted bytes, so an uncapped chain is verification amplification. The cap lives here as well as at the governance wire boundary because `calimero-op` has no bounds layer at all - relying on every caller to bound the field first would leave the unified plane open.
- **A handoff chain must start at epoch 0 and step by exactly one.** A gap would accept a key whose authorization was never demonstrated; a repeat would make "the key at epoch n" ambiguous. Each handoff is signed by the *outgoing* key, which is what makes the chain an authorization chain rather than a list of assertions.
- **There is no certificate expiry, on purpose.** Expiry needs participants to agree on wall-clock time, which a causally-ordered system does not provide - a cert expiring "at" a timestamp would be valid on one node and invalid on another, and authorization would stop converging. Withdrawal is a revocation op instead.
- **An account root is a member NOWHERE, on purpose.** It is kept offline so it survives losing every device, which is what makes recovery possible - so the link gate cannot ask "is the root a member?". `AccountMemberEndorsement` is how a granted member key vouches for the account instead. It takes both to enroll: only a member can endorse, only the root can certify a device, and neither alone is enough.
- **`verify_account_endorsement` is validity, not authority** - same split as `VerifiedDeviceCert` versus "in force". Whether the endorser is a member is an at-cut question for the projection. Anyone may endorse any account (ids are public) and it grants nothing.
- **The endorser is inside the signed payload.** Without it, swapping the `member` field would leave a signature verifying against a key that never signed - a member could be shown to have endorsed an account it never touched.
- **`DeviceId` is minted from a nonce, not from the device's keys**, so rotating a device's keypair keeps its replica identity - and therefore its counter slots and HLC lineage - intact.
- **HLC-seed collisions are resolved on READ, never at link time.** At most one of two devices sharing an `hlc_seed()` may be live in a scope (lower id wins), but which one cannot be decided as each link arrives: "is there a lower colliding id" reads only what has folded so far, so the live set would depend on delivery order. `ScopeState::live_devices` (and `AccountBindingRepository::live_bindings` on the governance path) apply the rule over the whole folded set instead.
- **The device's signing key and KEM key are separate types.** Reusing one Ed25519 key for both a signature scheme and a Diffie-Hellman is a known footgun with no compensating benefit; the type split makes passing one for the other impossible.
- **There is no derivation from a bare key to an account here, deliberately.** The transitional bridge needs one, and it lives private to `calimero-op-adapter` - the crate deleted at cutover. Offering it here would make a value with none of an account's properties (no rotatable root, no revocable devices) look first-class, quietly reintroducing the id-equals-key conflation this crate exists to remove.

Part of [crates/](../AGENTS.md).
