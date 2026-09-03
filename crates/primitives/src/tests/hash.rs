use serde_json::{from_value as from_json_value, json, to_string as to_json_string};

use super::*;

#[test]
fn zero_hash_matches_from_bytes() {
    let from_bytes: Hash = [0u8; 32].into();
    assert_eq!(Hash::zero(), from_bytes);
    // 64 zeros. Under base58 this rendered as 32 '1's — and "11" repeated is also
    // a plausible-looking hex id, which is exactly the collision that made a hex
    // value decode silently as base58 to all zeros.
    assert_eq!(from_bytes.to_string(), "0".repeat(64));
}

#[test]
fn test_hash_43() {
    let hash = Hash::new(b"Hello, World");
    let expected = "03675ac53ff9cd1535ccc7dfcdfa2c458c5218371f418dc136f2d19ac1fbe8a5";

    assert_eq!(hex::encode(hash.as_bytes()), expected);
    // Display IS the hex form — asserted against the same constant, so the two
    // cannot drift apart the way Display and `From<_> for String` once did.
    assert_eq!(hash.to_string(), expected);
}

#[test]
fn test_hash_44() {
    let hash = Hash::new(b"Hello World");
    let expected = "a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e";

    assert_eq!(hex::encode(hash.as_bytes()), expected);
    assert_eq!(hash.to_string(), expected);
}

#[test]
fn a_hash_round_trips_through_its_string_form() {
    let hash = Hash::new(b"Hello World");

    assert_eq!(hash.to_string().parse::<Hash>().expect("parse"), hash);
}

/// Base58 must now be **refused**, not quietly accepted.
///
/// Accepting both was right while two spellings were in circulation; with one, a
/// base58 value reaching a parser means a caller is still on the old form, and
/// silently decoding it hides that. This is the assertion that keeps leniency from
/// creeping back.
#[test]
fn base58_is_no_longer_accepted() {
    // The old rendering of `Hash::new(b"Hello World")`.
    let old = "C9K5weED8iiEgM6bkU6gZSgGsV6DW2igMtNtL1sjfFKK";

    assert!(
        old.parse::<Hash>().is_err(),
        "a base58 hash must be refused now that hex is the only form",
    );
}

/// Hex of the wrong length fails on length, not on the alphabet.
#[test]
fn a_short_hex_hash_is_refused() {
    assert!(matches!(
        "00ff".parse::<Hash>(),
        Err(HashError::InvalidLength)
    ));
}

#[test]
fn test_serde() {
    let hash = Hash::new(b"Hello World");
    let expected = "a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e";

    assert_eq!(to_json_string(&hash).unwrap(), format!("\"{expected}\""));
    assert_eq!(from_json_value::<Hash>(json!(expected)).unwrap(), hash);
}
