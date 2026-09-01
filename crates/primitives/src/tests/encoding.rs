//! One rule, pinned once: **every 32-byte id renders and parses as 64 hex.**
//!
//! Each id type already round-trips itself in its own module. That is not the
//! same guarantee: a type can round-trip perfectly while spelling its bytes
//! differently from every other id, which is the state this crate was in. The
//! per-type tests all passed the whole time.
//!
//! So this asserts the property those cannot — that the *same bytes* produce the
//! *same string* across all of them — and it is deliberately written as a loop
//! over types rather than N assertions, so adding an id type without adding it
//! here is the only way to escape the rule.
//!
//! Base58 refusal is checked alongside, and matters more than it looks. Hex
//! digits are a strict subset of the base58 alphabet, so a parser accepting both
//! would decode some strings under the wrong alphabet and silently yield the
//! wrong id rather than an error.

use crate::application::ApplicationId;
use crate::blobs::BlobId;
use crate::context::ContextId;
use crate::hash::Hash;
use crate::identity::{AccountId, DeviceId, PublicKey};

/// The 32 bytes every type below is asked to spell.
const BYTES: [u8; 32] = [0xAB; 32];

/// `BYTES` in the one encoding this crate now has.
const HEX: &str = "abababababababababababababababababababababababababababababababab";

/// `BYTES` in base58 — what these used to look like, and what must now be refused.
const BS58: &str = "CZ8YUVdk7znjrUmnb5n7kgySk9yRAsQDYmyCxzfSky9t";

/// Render every id type from identical bytes; they must agree, and agree on hex.
#[test]
fn every_id_type_spells_the_same_bytes_the_same_way() {
    let spellings = [
        ("Hash", Hash::from(BYTES).to_string()),
        ("ContextId", ContextId::from(BYTES).to_string()),
        ("ApplicationId", ApplicationId::from(BYTES).to_string()),
        ("BlobId", BlobId::from(BYTES).to_string()),
        ("PublicKey", PublicKey::from(BYTES).to_string()),
        ("AccountId", AccountId::from(BYTES).to_string()),
        ("DeviceId", DeviceId::from(BYTES).to_string()),
    ];

    for (name, rendered) in &spellings {
        assert_eq!(
            rendered, HEX,
            "{name} does not render its bytes as hex like every other id",
        );
    }
}

/// And parse back, from that one spelling.
#[test]
fn every_id_type_parses_the_one_spelling() {
    assert_eq!(HEX.parse::<Hash>().unwrap(), Hash::from(BYTES));
    assert_eq!(HEX.parse::<ContextId>().unwrap(), ContextId::from(BYTES));
    assert_eq!(
        HEX.parse::<ApplicationId>().unwrap(),
        ApplicationId::from(BYTES)
    );
    assert_eq!(HEX.parse::<BlobId>().unwrap(), BlobId::from(BYTES));
    assert_eq!(HEX.parse::<PublicKey>().unwrap(), PublicKey::from(BYTES));
    assert_eq!(HEX.parse::<AccountId>().unwrap(), AccountId::from(BYTES));
    assert_eq!(HEX.parse::<DeviceId>().unwrap(), DeviceId::from(BYTES));
}

/// Base58 is refused everywhere, rather than quietly decoded.
#[test]
fn no_id_type_still_accepts_base58() {
    assert!(BS58.parse::<Hash>().is_err(), "Hash still takes base58");
    assert!(BS58.parse::<ContextId>().is_err(), "ContextId");
    assert!(BS58.parse::<ApplicationId>().is_err(), "ApplicationId");
    assert!(BS58.parse::<BlobId>().is_err(), "BlobId");
    assert!(BS58.parse::<PublicKey>().is_err(), "PublicKey");
    assert!(BS58.parse::<AccountId>().is_err(), "AccountId");
    assert!(BS58.parse::<DeviceId>().is_err(), "DeviceId");
}

/// JSON agrees with `Display` — the wire is the surface clients actually see.
#[test]
fn json_uses_the_same_spelling_as_display() {
    let quoted = format!("\"{HEX}\"");

    assert_eq!(serde_json::to_string(&Hash::from(BYTES)).unwrap(), quoted);
    assert_eq!(
        serde_json::to_string(&ContextId::from(BYTES)).unwrap(),
        quoted
    );
    assert_eq!(
        serde_json::to_string(&ApplicationId::from(BYTES)).unwrap(),
        quoted
    );
    assert_eq!(serde_json::to_string(&BlobId::from(BYTES)).unwrap(), quoted);
    assert_eq!(
        serde_json::to_string(&PublicKey::from(BYTES)).unwrap(),
        quoted
    );
    assert_eq!(
        serde_json::to_string(&AccountId::from(BYTES)).unwrap(),
        quoted
    );
    assert_eq!(
        serde_json::to_string(&DeviceId::from(BYTES)).unwrap(),
        quoted
    );
}
