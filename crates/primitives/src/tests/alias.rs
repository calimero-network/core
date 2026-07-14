use serde_json::json;

use crate::alias::Alias;

#[test]
fn test_valid_alias_creation() {
    let alias: Alias<()> = "test-alias".parse().unwrap();
    assert_eq!(alias.as_str(), "test-alias");
}

#[test]
fn test_alias_length_limit() {
    // Valid: exactly 50 chars
    let valid = "a".repeat(50).parse::<Alias<()>>();
    let _ignored = valid.expect("valid alias");

    // Invalid: 51 chars
    let invalid = "a".repeat(51).parse::<Alias<()>>();
    let _ignored = invalid.expect_err("invalid alias");
}

#[test]
fn test_empty_alias_is_rejected() {
    // Empty aliases are not allowed.
    let err = "".parse::<Alias<()>>().expect_err("empty alias");
    assert!(err.to_string().contains("must not be empty"));
}

#[test]
fn test_allowed_punctuation() {
    // Dots, dashes, and underscores are permitted.
    let allowed = "test-123_v1.2";
    let alias: Alias<()> = allowed.parse().unwrap();
    assert_eq!(alias.as_str(), allowed);
}

#[test]
fn test_special_characters_are_rejected() {
    // Characters outside [A-Za-z0-9._-] are rejected, including path
    // separators and shell metacharacters.
    for bad in ["test@host", "a/b", "a b", "a\\b", "with#hash", "a$b"] {
        let err = bad
            .parse::<Alias<()>>()
            .expect_err("special characters should be rejected");
        assert!(
            err.to_string().contains("invalid character"),
            "unexpected error for {bad:?}: {err}"
        );
    }
}

#[test]
fn test_unicode_characters_are_rejected() {
    // Non-ASCII aliases are rejected.
    let err = "测试-алиас-🦀"
        .parse::<Alias<()>>()
        .expect_err("unicode alias");
    assert!(err.to_string().contains("invalid character"));
}

#[test]
fn test_reject_interior_nul() {
    // An interior NUL would be truncated by the store's fixed-width decode,
    // letting "a\0b" collide with "a" on round-trip. Reject it up front.
    let result = "a\0b".parse::<Alias<()>>();
    assert!(result.is_err());
    let result = "\0".parse::<Alias<()>>();
    assert!(result.is_err());
}

#[test]
fn test_reject_control_characters() {
    for bad in ["line\nbreak", "tab\there", "bell\u{7}"] {
        assert!(
            bad.parse::<Alias<()>>().is_err(),
            "expected {bad:?} to be rejected"
        );
    }
}

#[test]
fn test_deserialize_valid_alias() {
    let json = json!("valid-alias");
    let alias: Alias<()> = serde_json::from_value(json).unwrap();
    assert_eq!(alias.as_str(), "valid-alias");
}

#[test]
fn test_deserialize_invalid_length() {
    let long_string = "a".repeat(51);
    let json = json!(long_string);
    let result: Result<Alias<()>, _> = serde_json::from_value(json);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("exceeds maximum length"));
}

#[test]
fn test_serialize_deserialize_roundtrip() {
    let original: Alias<()> = "test-alias".parse().unwrap();
    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Alias<()> = serde_json::from_str(&serialized).unwrap();
    assert_eq!(original, deserialized);
}
