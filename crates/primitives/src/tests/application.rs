use std::str::FromStr;

use serde_json::{from_value as from_json_value, json, to_string as to_json_string};

use super::{AppKey, InvalidAppKey, InvalidSignerId, SignerId};

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
        format!("{}", signer_id),
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
// AppKey Tests
// -----------------------------------------------------------------------------

#[test]
fn test_app_key_new_valid() {
    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let app_key = AppKey::new("com.example.myapp", signer_id).unwrap();

    assert_eq!(app_key.app_id(), "com.example.myapp");
    assert_eq!(
        app_key.signer_id().as_str(),
        "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK"
    );
}

#[test]
fn test_app_key_new_empty_app_id_fails() {
    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let result = AppKey::new("", signer_id);
    assert!(matches!(result, Err(InvalidAppKey::EmptyAppId)));
}

#[test]
fn test_app_key_display() {
    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let app_key = AppKey::new("com.example.myapp", signer_id).unwrap();

    assert_eq!(
        format!("{}", app_key),
        "com.example.myapp:did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK"
    );
}

#[test]
fn test_app_key_from_str_valid() {
    let app_key = AppKey::from_str(
        "com.example.myapp:did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK",
    )
    .unwrap();

    assert_eq!(app_key.app_id(), "com.example.myapp");
    assert_eq!(
        app_key.signer_id().as_str(),
        "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK"
    );
}

#[test]
fn test_app_key_from_str_no_separator_fails() {
    let result = AppKey::from_str("com.example.myapp");
    assert!(matches!(result, Err(InvalidAppKey::InvalidFormat(_))));
}

#[test]
fn test_app_key_from_str_empty_app_id_fails() {
    let result = AppKey::from_str(":did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK");
    assert!(matches!(result, Err(InvalidAppKey::EmptyAppId)));
}

#[test]
fn test_app_key_from_str_empty_signer_id_fails() {
    let result = AppKey::from_str("com.example.myapp:");
    assert!(matches!(
        result,
        Err(InvalidAppKey::InvalidSignerId(InvalidSignerId::Empty))
    ));
}

#[test]
fn test_app_key_into_string() {
    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let app_key = AppKey::new("com.example.myapp", signer_id).unwrap();

    let s: String = app_key.into();
    assert_eq!(
        s,
        "com.example.myapp:did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK"
    );
}

#[test]
fn test_app_key_serde_roundtrip() {
    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let app_key = AppKey::new("com.example.myapp", signer_id).unwrap();

    // Serialize to JSON
    let json_str = to_json_string(&app_key).unwrap();
    assert_eq!(
        json_str,
        "\"com.example.myapp:did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK\""
    );

    // Deserialize from JSON
    let deserialized: AppKey = from_json_value(json!(
        "com.example.myapp:did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK"
    ))
    .unwrap();
    assert_eq!(app_key, deserialized);
}

#[test]
fn test_app_key_equality() {
    let s1 = SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let s2 = SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let s3 = SignerId::new("did:key:z6MkDifferent").unwrap();

    let k1 = AppKey::new("com.example.myapp", s1).unwrap();
    let k2 = AppKey::new("com.example.myapp", s2).unwrap();
    let k3 = AppKey::new("com.example.myapp", s3).unwrap();
    let k4 = AppKey::new(
        "com.example.otherapp",
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap(),
    )
    .unwrap();

    assert_eq!(k1, k2);
    assert_ne!(k1, k3); // Different signer
    assert_ne!(k1, k4); // Different app_id
}

#[test]
fn test_app_key_hash() {
    use std::collections::HashSet;

    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let k1 = AppKey::new("com.example.myapp", signer_id.clone()).unwrap();
    let k2 = AppKey::new("com.example.myapp", signer_id).unwrap();

    let mut set = HashSet::new();
    set.insert(k1.clone());
    assert!(set.contains(&k2));
}

#[test]
fn test_app_key_borsh_roundtrip() {
    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();
    let app_key = AppKey::new("com.example.myapp", signer_id).unwrap();

    // Serialize
    let bytes = borsh::to_vec(&app_key).unwrap();

    // Deserialize
    let deserialized: AppKey = borsh::from_slice(&bytes).unwrap();
    assert_eq!(app_key, deserialized);
}

#[test]
fn test_app_key_display_roundtrip() {
    // Test that Display -> FromStr is a roundtrip
    let original_str = "com.example.myapp:did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK";
    let app_key = AppKey::from_str(original_str).unwrap();
    let display_str = app_key.to_string();
    assert_eq!(original_str, display_str);
}

#[test]
fn test_app_key_complex_app_id() {
    // Test with various valid app_id formats
    let signer_id =
        SignerId::new("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK").unwrap();

    // Simple name
    let k1 = AppKey::new("myapp", signer_id.clone()).unwrap();
    assert_eq!(k1.app_id(), "myapp");

    // Reverse domain format
    let k2 = AppKey::new("com.calimero.kv-store", signer_id.clone()).unwrap();
    assert_eq!(k2.app_id(), "com.calimero.kv-store");

    // With underscores and numbers
    let k3 = AppKey::new("my_app_v2", signer_id).unwrap();
    assert_eq!(k3.app_id(), "my_app_v2");
}
