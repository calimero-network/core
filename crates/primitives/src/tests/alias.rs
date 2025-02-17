#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::alias::Alias;

    #[test]
    fn test_valid_alias_creation() {
        let alias = Alias::try_from("test-alias".to_string()).unwrap();
        assert_eq!(alias.as_str(), "test-alias");
    }

    #[test]
    fn test_alias_length_limit() {
        // Valid: exactly 50 chars
        let valid = "a".repeat(50);
        assert!(Alias::try_from(valid).is_ok());

        // Invalid: 51 chars
        let invalid = "a".repeat(51);
        assert!(Alias::try_from(invalid).is_err());
    }

    #[test]
    fn test_from_str() {
        // Valid case
        let alias: Alias = "test-alias".parse().unwrap();
        assert_eq!(alias.as_str(), "test-alias");

        // Invalid case (too long)
        let result: Result<Alias, _> = "a".repeat(51).parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_try_from_string() {
        // Valid case
        let valid_string = "valid-alias".to_string();
        let alias = Alias::try_from(valid_string).unwrap();
        assert_eq!(alias.as_str(), "valid-alias");

        // Invalid case
        let invalid_string = "a".repeat(51);
        assert!(Alias::try_from(invalid_string).is_err());
    }

    #[test]
    fn test_conversion_to_string() {
        let alias = Alias::try_from("convert-test".to_string()).unwrap();
        // Test From<Alias> for String
        let string: String = alias.into();
        assert_eq!(string, "convert-test");
    }

    #[test]
    fn test_empty_alias() {
        // Empty string should be valid
        let alias = Alias::try_from(String::new()).unwrap();
        assert_eq!(alias.as_str(), "");
    }

    #[test]
    fn test_special_characters() {
        // Test with various special characters
        let special = "test-123_@#$%^&*()".to_string();
        let alias = Alias::try_from(special.clone()).unwrap();
        assert_eq!(alias.as_str(), special);
    }

    #[test]
    fn test_unicode_characters() {
        // Test with Unicode characters
        let unicode = "测试-алиас-🦀".to_string();
        let alias = Alias::try_from(unicode.clone()).unwrap();
        assert_eq!(alias.as_str(), unicode);
    }

    #[test]
    fn test_deserialize_valid_alias() {
        let json = json!("valid-alias");
        let alias: Alias = serde_json::from_value(json).unwrap();
        assert_eq!(alias.as_str(), "valid-alias");
    }

    #[test]
    fn test_deserialize_invalid_length() {
        let long_string = "a".repeat(51);
        let json = json!(long_string);
        let result: Result<Alias, _> = serde_json::from_value(json);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("exceeds maximum length"));
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let original = Alias::try_from("test-alias".to_string()).unwrap();
        let serialized = serde_json::to_string(&original).unwrap();
        let deserialized: Alias = serde_json::from_str(&serialized).unwrap();
        assert_eq!(original, deserialized);
    }
}
