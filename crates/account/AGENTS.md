# calimero-account - Account Identity Primitive

The two stable ids that let one person hold several devices: `AccountId` (who) and `DeviceId` (which replica), plus the self-certifying credentials that bind them.

## Package Identity

- **Crate**: `calimero-account`
- **Entry**: `src/lib.rs` (crate docs + the flat re-export facade; the model lives in the modules below)
- **Key deps**: `borsh` (wire format), `hex` (key display), `thiserror`, `calimero-primitives` (`PublicKey`/`AccountId`/`DeviceId`, with the `borsh` feature)

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

## How the pieces interact

**What hashes to what, and who signs what.** Unlabelled arrows derive a value from the one above; labelled edges name the relation. The account root is the only key that certifies devices, and it is deliberately a member nowhere:

```text
   root_sk  (offline; survives losing every device)
     │
     │ names the epoch-0 key
     ▼
   AccountGenesis {version, root_sign_pk}
     │
     │ H(domain ‖ borsh)                     DeviceId::mint(account, nonce_16)
     ▼                                                    │
   AccountId ─────────────────────────────────────────────┴──▶ DeviceId
     ▲            (the only authz subject)                       │
     │                                                           └─▶ hlc_seed()  = first 16 bytes
     │ covers                                                           (CRDT replica id + HLC seed)
   AccountMemberEndorsement  ◀── signed by a GRANTED MEMBER key, never by the root
     └── .verify() ──▶ VerifiedEndorsement   (a gate reads `member` from HERE, not from the
                                              unchecked struct — that is the point of it)

   root-key epochs, each handoff signed by the OUTGOING key (an authorization chain, not a list):

   pk_0 ──RootKeyHandoff──▶ pk_1 ──RootKeyHandoff──▶ pk_2 ─── … (capped at MAX_ROOT_KEY_HANDOFFS)
     └── root_key_at_epoch(genesis, chain, e) walks it as far as `e` and stops ──▶ pk_e
                                                                                   │ signs
                                    ┌──────────────────────────────────────────────┤
                                    ▼                                              ▼
   DeviceCert {account, device, sign_pk, kem_pk, key_epoch=e, device_epoch}   DeviceRevocation
                                                                             {account, device, e}
```

**Linking a device, end to end** - and where this crate stops:

```text
 new device                                account holder (root_sk offline + a granted member key)
 ──────────                                ─────────────────────────────────────────────────────
 mint sign_pk + kem_pk
 DeviceId::mint(account, nonce)
 PairingOffer::signed(sk, …) ─────────────▶ offer.verify_statement(sig)  refuses a PARTIAL key swap
   → (offer, statement)                        │
 offer.confirmation_code() ──human reads──▶  offer.code_matches(typed)   refuses a WHOLESALE swap
                                                       │ both pass
                                                       ▼
                                             DeviceCert::sign(root_sk, …)
                                             AccountMemberEndorsement::sign(member_sk, account)
                                                       │
                                                       ▼
                                            ONE self-contained link op ──gossip──┐
                                                                                 │
 every receiver, with no prior state and no ordering dependency:  ◀───────────────┘
   verify_device_cert(op.author, genesis, chain, cert) → Verified<DeviceCert>
   endorsement.verify()                                → Verified<AccountMemberEndorsement>
                          │  internally valid - NOT "in force"
 ═════════════════════════▼══════════ crate boundary ══════════════════════════════
 calimero-projection / calimero-authz answer what only the causal cut can:
   is key_epoch superseded?   is the device revoked?   is the endorser a member here?
   of two devices sharing an hlc_seed, which is live?  (lower DeviceId, decided on read)
```

**Module map.** Dependencies run one way, so a change to the anchor cannot be shadowed by a change to a credential:

```text
   pairing.rs ──▶ device.rs ────┐
                                ├──▶ root_key.rs ──▶ account.rs ──▶ domain.rs
   revocation.rs ───────────────┘     (chain walk)    (the anchor    (every signing domain,
        │              │                              + borsh        pairwise-distinct)
        └──────────────┴────────▶ signed.rs            preimages)
                                  (RootSigned, Verified<T>, AccountProof<T>,
                                   verify_root_signed, sign_payload — the one
                                   verifier and the one bundle both statements share)

   error.rs ◀── every fallible path in all of the above returns AccountError
```

`signed.rs` is where the shape lives, not a helper dump: `verify_root_signed` is the
*only* end-to-end verifier, and `device.rs` / `revocation.rs` each add just their
statement's fields and their two error variants. `sign_payload` is likewise the one
signing tail every minter ends in.

`pairing.rs` reaches into `device.rs` for `KemPublicKey` alone - it certifies nothing itself. `revocation.rs` does *not* go through `device.rs`: a revocation is verified against the key chain directly, which is why it stays valid under any epoch the chain resolves while a certificate's superseded epochs get filtered on read.

## Public API

Three shapes carry the whole crate: a **statement** is plain signed data, a
**proof** is a statement plus the anchor and chain needed to check it standalone,
and a `Verified<T>` is one that has been checked.

| Item | Kind | Purpose |
| --- | --- | --- |
| `AccountId` | struct (`[u8; 32]`) | Content address of an `AccountGenesis`; the only authorization subject |
| `DeviceId` | struct (`[u8; 32]`) | One installation; the CRDT replica id |
| `DeviceId::mint(account, nonce)` | fn | Mint a device id once per installation |
| `DeviceId::hlc_seed()` | fn | First 16 bytes - the HLC instance seed for this replica |
| `KemPublicKey` | struct (`[u8; 32]`) | X25519 scope-key delivery recipient; a distinct type from `PublicKey` |
| `AccountGenesis` | struct | `{version, root_sign_pk}`; hashing it yields the `AccountId`. No per-scope salt - one root key is one account everywhere |
| `AccountGenesis::account_id()` | fn | The id this genesis addresses |
| `ACCOUNT_GENESIS_VERSION` | const | Version written into a genesis; part of the id preimage |
| **`RootSigned`** | trait | The shape a statement the account **root** signs shares: `account`, `key_epoch`, `payload`, `signature`, plus the two `AccountError` variants it reports. Implemented by `DeviceCert` and `DeviceRevocation`; deliberately **not** by `AccountMemberEndorsement` |
| **`Verified<T>`** | struct | A statement whose anchor, chain and signature all checked. Derefs to `T`; unconstructible outside this crate, so holding one *is* the proof a check happened. **Not** a statement that the credential is in force |
| **`AccountProof<T>`** | struct | `{genesis, chain, statement}` - a credential that stands on its own. The wire form; borsh-identical to the three loose fields it replaced |
| `AccountProof::verify(claimed_account)` | fn | Check a proof against an account the caller already trusts; yields `Verified<T>` |
| `RootKeyHandoff` | struct | Rolls the root key from `from_epoch` to `from_epoch + 1`, signed by the outgoing key |
| `RootKeyHandoff::sign(sk, account, from_epoch, new_pk)` | fn | Mint one |
| `root_key_at_epoch(genesis, chain, epoch)` | fn | Walk the chain as far as `epoch` and return the root key there; entries beyond it are never read |
| `MAX_ROOT_KEY_HANDOFFS` | const | `1024`; the chain cap, applied before any verification |
| `DeviceCert` | struct | Root-signed grant binding a device to an account |
| `DeviceCert::sign(root_sk, …)` | fn | Mint one - one parameter per signed field, deliberately not a builder |
| `verify_device_cert(claimed, genesis, chain, cert)` | fn | Full credential check; yields `VerifiedDeviceCert` |
| `VerifiedDeviceCert` | alias | `Verified<DeviceCert>` |
| `DeviceRevocation` | struct | Root-signed withdrawal of a device |
| `DeviceRevocation::sign(root_sk, account, device, key_epoch)` | fn | Mint one |
| `SignedDeviceRevocation` | alias | `AccountProof<DeviceRevocation>` - the wire-carried proof |
| `SignedDeviceRevocation::authorises(account, device)` | fn | Whether this proof authorises withdrawing *that* device; checks the device before spending an Ed25519 verification |
| `verify_device_revocation(claimed, genesis, chain, revocation)` | fn | Check against a borrowed chain; yields `VerifiedDeviceRevocation` |
| `VerifiedDeviceRevocation` | alias | `Verified<DeviceRevocation>` |
| `AccountMemberEndorsement` | struct | A granted member key's signed statement that an account is theirs |
| `AccountMemberEndorsement::sign(member_sk, account)` | fn | Mint one; the endorser is derived from the key, never named by the caller |
| `AccountMemberEndorsement::verify()` | fn | Internal validity only; yields `VerifiedEndorsement`, which is where a gate reads the endorser's key from |
| `VerifiedEndorsement` | alias | `Verified<AccountMemberEndorsement>` |
| `PairingOffer` | struct | `{account, device, kem_pk, sign_pk}` - the key material a pairing device minted, and every question either end asks about it |
| `PairingOffer::signed(device_sk, account, device, kem_pk)` | fn | The pairing side's constructor: returns `(offer, statement)`. Requires the secret, so possession is proved rather than asserted |
| `PairingOffer::new(…)` | fn | The verifying side's constructor, over key material that arrived |
| `PairingOffer::verify_statement(sig)` | fn | Refuses a **partial** key substitution |
| `PairingOffer::confirmation_code()` / `code_matches(supplied)` | fn | The 64-bit human-compared code; refuses a **wholesale** substitution |
| `AccountError` | enum | Why a credential failed |

`verify_root_signed`, the one generic behind `verify_device_cert` and
`verify_device_revocation`, is **crate-private**: nothing outside needs it, and an
unused `pub fn` in a repo that forbids dead code is a claim about an API nobody
asked for. Re-export it when something needs it.

## Key Files

Every public item is re-exported flat from `src/lib.rs`, so `calimero_account::DeviceCert` works regardless of which module it lives in — the modules are private and exist for readability, not as API surface.

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Crate docs (the WHY), module declarations, and the flat `pub use` facade |
| `src/account.rs` | `ACCOUNT_GENESIS_VERSION`, `AccountGenesis`, `AccountMemberEndorsement` + `sign`/`verify`, `VerifiedEndorsement`, and `borsh_bytes` (the id preimage helper, beside its only production caller) |
| `src/signed.rs` | The shared shape: `RootSigned`, `Verified<T>`, `AccountProof<T>`, `verify_root_signed` (the one verifier), `sign_payload` (the one signing tail) |
| `src/root_key.rs` | `MAX_ROOT_KEY_HANDOFFS`, `RootKeyHandoff` + `sign`, `root_key_at_epoch` (the chain walk) |
| `src/device.rs` | `KemPublicKey`, `DeviceCert` + `sign`, `VerifiedDeviceCert`, `verify_device_cert` |
| `src/revocation.rs` | `DeviceRevocation` + `sign`, `SignedDeviceRevocation` (= `AccountProof<DeviceRevocation>`), `authorises`, `verify_device_revocation` |
| `src/pairing.rs` | `PairingOffer` - the four values a pairing is about, and every question either end asks of them |
| `src/domain.rs` | Every signing/content-address domain in one place, so `signing_domains_are_pairwise_distinct` is a check over the whole set |
| `src/error.rs` | `AccountError` |
| `src/tests.rs` | Declares the test tree; every test in the crate lives under `src/tests/` |
| `src/tests/<module>.rs` | Tests for the module of the same name, reaching it through `crate::` paths |
| `src/tests/wire.rs` | Cross-cutting: borsh round-trips, plus `recorded_before_the_refactor` - hex bytes captured on the pre-`AccountProof` tree, so a field reorder is caught as the wire break it is |
| `src/tests/signed.rs` | That both verify entry points agree, that each statement kind still reports its own errors, and that the device check precedes the signature check |
| `src/tests/support.rs` | Shared fixtures (`key`, `genesis_for`, `rotated`, `sign_handoff`, `sign_cert`, `pairing_fixture`) |

## Invariants and Gotchas

- **`VerifiedDeviceCert` does not mean "in force".** It means the credential is internally valid. Supersession, revocation and scope membership are at-cut questions answered by `calimero-authz`. The type exists so the two stages cannot be confused.
- **Signing domains must stay pairwise distinct.** Every content address and signing preimage goes through `domain_hash`, which **length-prefixes** the domain - a bare concatenation would let a shorter domain that prefixes a longer one collide by shifting bytes across the boundary. `signing_domains_are_pairwise_distinct` and `domain_hash_is_not_confusable_by_shifting_bytes` guard this.
- **`root_key_at_epoch` caps the chain at `MAX_ROOT_KEY_HANDOFFS` (1024) before reading anything.** Each entry costs an Ed25519 verification and the function is reachable from untrusted bytes, so an uncapped chain is verification amplification. The cap lives here as well as at the governance wire boundary because `calimero-op` has no bounds layer at all - relying on every caller to bound the field first would leave the unified plane open.
- **The walk stops at the epoch asked for; entries beyond it are neither read nor verified.** They are not part of the authorization the credential rests on, so letting one refuse the whole credential would invalidate a certificate that verifies perfectly against a key the chain genuinely established. It also means a credential at epoch 0 costs zero verifications instead of up to 1024 - the cap bounds that work, it does not avoid doing it. Nothing is gained by appending junk: reaching the epoch a junk handoff claims to establish requires verifying every entry up to it.
- **A handoff chain must start at epoch 0 and step by exactly one.** A gap would accept a key whose authorization was never demonstrated; a repeat would make "the key at epoch n" ambiguous. Each handoff is signed by the *outgoing* key, which is what makes the chain an authorization chain rather than a list of assertions.
- **There is no certificate expiry, on purpose.** Expiry needs participants to agree on wall-clock time, which a causally-ordered system does not provide - a cert expiring "at" a timestamp would be valid on one node and invalid on another, and authorization would stop converging. Withdrawal is a revocation op instead.
- **An account root is a member NOWHERE, on purpose.** It is kept offline so it survives losing every device, which is what makes recovery possible - so the link gate cannot ask "is the root a member?". `AccountMemberEndorsement` is how a granted member key vouches for the account instead. It takes both to enroll: only a member can endorse, only the root can certify a device, and neither alone is enough.
- **`AccountMemberEndorsement::verify` is validity, not authority** - same split as `Verified<T>` versus "in force". Whether the endorser is a member is an at-cut question for the projection. Anyone may endorse any account (ids are public) and it grants nothing. It returns `VerifiedEndorsement` rather than `Result<(), _>` precisely so a gate reads the endorser's key off the wrapper instead of off a struct nobody checked.
- **`AccountMemberEndorsement` does not implement `RootSigned`, deliberately.** It is signed by a granted *member* key, not the account root - which is the entire reason it exists. That difference used to live only in prose; keeping it outside the trait puts it in the type system.
- **`Verified<T>` cannot be built outside this crate.** Its field is private and every constructor sits behind a check. If you need to fabricate one in a test, that is a signal the check belongs in the test too - not a reason to widen the constructor.
- **`AccountProof<T>` is borsh-identical to the loose fields it replaced.** Field order is `genesis, chain, statement`, matching the old `JoinAccountCredential { genesis, chain, cert }` and `SignedDeviceRevocation { genesis, chain, revocation }` exactly. `recorded_before_the_refactor` pins it against bytes captured before the change - if it fails, the encoding moved and that is a wire break needing a version bump, not a test to update.
- **The end-to-end verifiers take a BORROWED chain on purpose.** `verify_device_cert` / `verify_device_revocation` are called from apply paths that hold a `&[RootKeyHandoff]`; making them methods on `AccountProof` would force those callers to allocate a proof per check just to discard it. A caller that already *has* a proof should use `AccountProof::verify`.
- **The endorser is inside the signed payload.** Without it, swapping the `member` field would leave a signature verifying against a key that never signed - a member could be shown to have endorsed an account it never touched.
- **`DeviceId` is minted from a nonce, not from the device's keys**, so rotating a device's keypair keeps its replica identity - and therefore its counter slots and HLC lineage - intact.
- **HLC-seed collisions are resolved on READ, never at link time.** At most one of two devices sharing an `hlc_seed()` may be live in a scope (lower id wins), but which one cannot be decided as each link arrives: "is there a lower colliding id" reads only what has folded so far, so the live set would depend on delivery order. `ScopeState::live_devices` (and `AccountBindingRepository::live_bindings` on the governance path) apply the rule over the whole folded set instead.
- **The device's signing key and KEM key are separate types.** Reusing one Ed25519 key for both a signature scheme and a Diffie-Hellman is a known footgun with no compensating benefit; the type split makes passing one for the other impossible.
- **There is no derivation from a bare key to an account here, deliberately.** The transitional bridge needs one, and it lives private to `calimero-op-adapter` - the crate deleted at cutover. Offering it here would make a value with none of an account's properties (no rotatable root, no revocable devices) look first-class, quietly reintroducing the id-equals-key conflation this crate exists to remove.

Part of [crates/](../AGENTS.md).
