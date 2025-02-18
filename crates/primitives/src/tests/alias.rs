#[cfg(test)]
mod tests {
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
    fn test_empty_alias() {
        // Empty string should be valid
        let alias: Alias<()> = "".parse().unwrap();
        assert_eq!(alias.as_str(), "");
    }

    #[test]
    fn test_special_characters() {
        // Test with various special characters
        let special = "test-123_@#$%^&*()";
        let alias: Alias<()> = special.parse().unwrap();
        assert_eq!(alias.as_str(), special);
    }

    #[test]
    fn test_unicode_characters() {
        // Test with Unicode characters
        let unicode = "测试-алиас-🦀";
        let alias: Alias<()> = unicode.parse().unwrap();
        assert_eq!(alias.as_str(), unicode);
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
}
