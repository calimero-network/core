use std::str::FromStr;

use serde_json::{from_value as from_json_value, json, to_string as to_json_string};

use super::{ApplicationId, InvalidSignerId, SignerId};
use crate::hash::Hash;

// -----------------------------------------------------------------------------
// SignerId Tests
// -----------------------------------------------------------------------------

#[test]
fn test_signer_id_new_valid() {
    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    assert_eq!(
        signer_id.as_str(),
        "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK"
    );
}

#[test]
fn test_signer_id_new_empty_fails() {
    let result = SignerId::new("");
    assert!(matches!(result, Err(InvalidSignerId::Empty)));
}

#[test]
fn test_signer_id_from_str_valid() {
    let signer_id =
        SignerId::from_str("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    assert_eq!(
        signer_id.as_str(),
        "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK"
    );
}

#[test]
fn test_signer_id_from_str_empty_fails() {
    let result = SignerId::from_str("");
    assert!(matches!(result, Err(InvalidSignerId::Empty)));
}

#[test]
fn test_signer_id_display() {
    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    assert_eq!(
        format!("{signer_id}"),
        "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK"
    );
}

#[test]
fn test_signer_id_into_string() {
    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let s: String = signer_id.into();
    assert_eq!(
        s,
        "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK"
    );
}

#[test]
fn test_signer_id_serde_roundtrip() {
    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();

    // Serialize to JSON
    let json_str = to_json_string(&signer_id).unwrap();
    assert_eq!(
        json_str,
        "\"did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK\""
    );

    // Deserialize from JSON
    let deserialized: SignerId = from_json_value(json!(
        "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK"
    ))
    .unwrap();
    assert_eq!(signer_id, deserialized);
}

#[test]
fn test_signer_id_deserialize_empty_fails() {
    let result = from_json_value::<SignerId>(json!(""));
    assert!(result.is_err());
}

#[test]
fn test_signer_id_equality() {
    let s1 = SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let s2 = SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let s3 = SignerId::new("did:key:z6MkDifferent").unwrap();

    assert_eq!(s1, s2);
    assert_ne!(s1, s3);
}

#[test]
fn test_signer_id_hash() {
    use std::collections::HashSet;

    let s1 = SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let s2 = SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();

    let mut set = HashSet::new();
    set.insert(s1.clone());
    assert!(set.contains(&s2));
}

#[test]
fn test_signer_id_borsh_roundtrip() {
    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();

    // Serialize
    let bytes = borsh::to_vec(&signer_id).unwrap();

    // Deserialize
    let deserialized: SignerId = borsh::from_slice(&bytes).unwrap();
    assert_eq!(signer_id, deserialized);
}

// -----------------------------------------------------------------------------
// ApplicationId::for_bundle Tests
// -----------------------------------------------------------------------------

#[test]
fn for_bundle_matches_the_inline_derivation() {
    let package: Box<str> = "com.example.demo".into();
    let signer: Box<str> = "did:key:z6MkExample".into();
    let inline = ApplicationId::from(*Hash::hash_borsh(&(&package, &signer)).expect("hash"));
    assert_eq!(
        ApplicationId::for_bundle(&package, &signer).expect("helper"),
        inline
    );
}
