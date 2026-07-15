# calimero-crypto - Cryptographic Utilities

ECDH-derived shared-key encryption for peer-to-peer sync traffic, built on Curve25519 and AES-256-GCM.

## Package Identity

- **Crate**: `calimero-crypto`
- **Entry**: `src/lib.rs`
- **Key deps**: `curve25519-dalek` (Edwards point arithmetic), `ed25519-dalek` (private key -> scalar), `ring` (HKDF-SHA256, AES-256-GCM AEAD), `zeroize` (key material wiping), `calimero-primitives` (`PrivateKey`/`PublicKey` types)

## Commands

```bash
# Build
cargo build -p calimero-crypto

# Test (all)
cargo test -p calimero-crypto

# Test a single case
cargo test -p calimero-crypto test_kdf_derivation_is_deterministic_and_interoperable -- --nocapture
```

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `SharedKey` | struct | Holds a zeroized 32-byte AEAD key; `Clone`, `Zeroize`, `ZeroizeOnDrop`, redacted `Debug` |
| `SharedKey::new(sk, pk)` | fn | X25519-style ECDH between a `PrivateKey` and peer `PublicKey`, HKDF-derived into an AES key |
| `SharedKey::from_sk(sk)` | fn | Uses the raw private key bytes directly as the AEAD key (no ECDH) |
| `SharedKey::encrypt(payload)` | fn | AES-256-GCM seal with an internally generated random nonce; returns `(Nonce, Vec<u8>)` |
| `SharedKey::encrypt_with_nonce(payload, nonce)` | fn | Seal with a caller-supplied nonce; caller must guarantee single-use |
| `SharedKey::decrypt(cipher_text, nonce)` | fn | AES-256-GCM open; returns `None` on any authentication failure |
| `SharedKeyError` | enum (`#[non_exhaustive]`) | Currently one variant: `InvalidPublicKey` |
| `NONCE_LEN` | const | `12` (AES-GCM standard nonce size) |
| `Nonce` | type alias | `[u8; NONCE_LEN]` |

All fallible AEAD operations return `Option`, not `Result` - there is no distinction exposed between "bad key," "bad nonce," and "tampered ciphertext"; all collapse to `None`.

## Mental Model

`SharedKey::new` is the normal path: it treats `sk` as an Ed25519 signing key, converts it to its underlying scalar (`SigningKey::to_scalar`), decompresses the peer's Edwards Y-coordinate public key, and multiplies scalar * point to get a raw ECDH secret. A raw curve point is not uniformly distributed over 256 bits, so it is never used directly as a key - it is fed as IKM into HKDF-SHA256 (`hkdf::Salt::new` with an empty salt, then `expand` with the fixed info string `AEAD_KDF_INFO = b"calimero.sharedkey.aead.v2"`) to produce the actual 32-byte AES-256-GCM key. Both peers derive the same key because ECDH is commutative: `signer_sk * verifier_pk == verifier_sk * signer_pk`.

Before doing any of that, `new` rejects public keys that decompress to a small-order (torsion) point via `is_small_order()`. A small-order peer key would collapse the ECDH output into a tiny subgroup independent of the caller's own scalar, defeating the "shared" part of the secret - this is the standard X25519/Ed25519 small-subgroup attack guard.

`SharedKey::from_sk` is a separate, non-ECDH path: it just wraps the private key's own bytes as the AES key. It is used where the "shared key" is really a single party's own symmetric secret rather than a peer-derived one - check callers before assuming ECDH semantics apply.

Nonce handling is asymmetric by design: `encrypt` generates its own random nonce (avoiding caller-side nonce reuse, which is catastrophic for AES-GCM), while `encrypt_with_nonce` exists for protocols that need to control the nonce themselves (e.g. a per-message ratchet), pushing the single-use guarantee onto the caller.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Everything: `SharedKey`, `SharedKeyError`, `Nonce`/`NONCE_LEN`, and all tests |

There is no module split - the whole crate is ~400 lines in one file, about half of it tests.

## Invariants and Gotchas

- **Zeroization**: `SharedKey.key` is `Zeroizing<[u8; 32]>`; the ECDH scalar and raw ECDH point bytes inside `new` are also wrapped in `Zeroizing` so intermediate secrets don't linger in memory after the function returns. Do not add a manual `Drop` impl for `SharedKey` - `ZeroizeOnDrop` plus the `Zeroizing` field already handle it; a second wipe would double-zeroize harmlessly but is dead code.
- **`Debug` is redacted**: never remove the custom `impl Debug for SharedKey` - the derived one would print key bytes.
- **HKDF info string is versioned**: `AEAD_KDF_INFO` ends in `.v2`. If the derivation ever changes (salt, info, hash), bump the suffix so old and new derivations can never silently collide.
- **Small-order rejection is required, not defensive fluff**: skipping `is_small_order()` reintroduces a known subgroup-confinement attack against Curve25519-based ECDH.
- **Nonce reuse is caller-checked, not library-checked**: `encrypt_with_nonce` trusts the caller. Only use it where single-use is already guaranteed elsewhere (e.g. a monotonic ratchet), otherwise use `encrypt`.
- **`AES_256_GCM` key construction can fail** (`aead::UnboundKey::new(...).ok()?`) only if the key length is wrong, which cannot happen given the fixed `[u8; 32]` - the `Option` plumbing exists mainly for decrypt-time authentication failures, not key-setup failures.

Part of [crates/](../AGENTS.md).
