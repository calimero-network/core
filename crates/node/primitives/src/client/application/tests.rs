//! Tests for the application module (bundle validation, path safety).

use std::cmp::Ordering;
use std::sync::Arc;

use super::{bundle, compare_versions};

#[test]
fn compare_versions_orders_semver_not_strings() {
    assert_eq!(compare_versions("1.10.0", "1.9.0"), Ordering::Greater);
    assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
    assert_eq!(compare_versions("1.0.0-rc.1", "1.0.0"), Ordering::Less);
}

#[test]
fn compare_versions_puts_an_unparseable_version_below_any_semver() {
    assert_eq!(compare_versions("nightly", "0.0.1"), Ordering::Less);
    assert_eq!(compare_versions("0.0.1", "nightly"), Ordering::Greater);
    assert_eq!(compare_versions("b", "a"), Ordering::Greater);
}

#[test]
fn test_validate_path_component_valid() {
    let valid_paths = vec!["com.example.app", "my-app", "my_app_v2", "app123"];
    for path in valid_paths {
        assert!(
            bundle::validate_path_component(path, "test").is_ok(),
            "Valid path '{path}' should pass validation"
        );
    }
}

#[test]
fn test_validate_path_component_path_traversal() {
    let invalid_paths = vec!["../etc", "..", "foo/../bar", "package..name"];
    for path in invalid_paths {
        assert!(
            bundle::validate_path_component(path, "test").is_err(),
            "Path traversal '{path}' should be rejected"
        );
    }
}

#[test]
fn test_validate_path_component_directory_separators() {
    let invalid_paths = vec!["foo/bar", "foo\\bar", "/absolute", "\\windows"];
    for path in invalid_paths {
        assert!(
            bundle::validate_path_component(path, "test").is_err(),
            "Path with separator '{path}' should be rejected"
        );
    }
}

#[test]
fn test_validate_path_component_null_byte() {
    let invalid_path = "package\0name";
    assert!(
        bundle::validate_path_component(invalid_path, "test").is_err(),
        "Path with null byte should be rejected"
    );
}

#[test]
fn test_validate_path_component_windows_drive() {
    let invalid_paths = vec!["C:malicious", "D:path"];
    for path in invalid_paths {
        assert!(
            bundle::validate_path_component(path, "test").is_err(),
            "Windows drive path '{path}' should be rejected"
        );
    }
}

#[test]
fn test_validate_path_component_unicode_separator() {
    // Test Unicode path separator (full-width slash)
    let _invalid_path = "package／name";
    // Note: This might pass current validation, but documents the limitation
    // The current implementation checks for ASCII '/' and '\' only
}

#[test]
fn test_validate_artifact_path_valid() {
    let valid_paths = vec!["app.wasm", "src/main.wasm", "migrations/001_init.sql"];
    for path in valid_paths {
        assert!(
            bundle::validate_artifact_path(path, "test").is_ok(),
            "Valid artifact path '{path}' should pass validation"
        );
    }
}

#[test]
fn test_validate_artifact_path_empty() {
    assert!(
        bundle::validate_artifact_path("", "test").is_err(),
        "Empty path should be rejected"
    );
}

#[test]
fn test_validate_artifact_path_null_byte() {
    let invalid_path = "app\0.wasm";
    assert!(
        bundle::validate_artifact_path(invalid_path, "test").is_err(),
        "Path with null byte should be rejected"
    );
}

#[test]
fn test_validate_artifact_path_backslash() {
    let invalid_path = "app\\main.wasm";
    assert!(
        bundle::validate_artifact_path(invalid_path, "test").is_err(),
        "Path with backslash should be rejected"
    );
}

#[test]
fn test_validate_artifact_path_absolute_unix() {
    let invalid_path = "/etc/passwd";
    assert!(
        bundle::validate_artifact_path(invalid_path, "test").is_err(),
        "Absolute Unix path should be rejected"
    );
}

#[test]
fn test_validate_artifact_path_absolute_windows() {
    let invalid_paths = vec!["C:malicious", "D:path\\file.wasm"];
    for path in invalid_paths {
        assert!(
            bundle::validate_artifact_path(path, "test").is_err(),
            "Windows absolute path '{path}' should be rejected"
        );
    }
}

#[test]
fn test_validate_artifact_path_traversal() {
    let invalid_paths = vec!["../etc/passwd", "foo/../bar", "..", "migrations/../../etc"];
    for path in invalid_paths {
        assert!(
            bundle::validate_artifact_path(path, "test").is_err(),
            "Path traversal '{path}' should be rejected"
        );
    }
}

#[test]
fn test_validate_artifact_path_url_encoded() {
    // Test URL-encoded path traversal attempts
    let _invalid_path = "..%2Fetc";
    // Note: Current implementation doesn't decode URL encoding
    // This test documents that URL-encoded sequences would need to be decoded first
}

#[test]
fn test_validate_artifact_path_very_long() {
    let long_path = "a".repeat(10000);
    assert!(
        bundle::validate_artifact_path(&long_path, "test").is_ok(),
        "Very long path currently passes validation (no length check implemented)"
    );
}

// -----------------------------------------------------------------------
// is_bundle_blob - non-bundle data
// -----------------------------------------------------------------------

#[test]
fn test_is_bundle_blob_random_bytes() {
    assert!(!bundle::is_bundle_blob(b"not a tar archive"));
    assert!(!bundle::is_bundle_blob(b""));
    assert!(!bundle::is_bundle_blob(&[0xFF; 100]));
}

// -----------------------------------------------------------------------
// extract_bundle_manifest - edge cases
// -----------------------------------------------------------------------

#[test]
fn test_extract_manifest_not_a_tar() {
    let result = bundle::extract_bundle_manifest(b"not a tar");
    assert!(result.is_err());
}

#[test]
fn test_extract_manifest_empty_tar() {
    // Create a valid but empty gzipped tar
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    // Write empty tar (just end-of-archive markers)
    encoder.write_all(&[0u8; 1024]).unwrap();
    let data = encoder.finish().unwrap();

    let result = bundle::extract_bundle_manifest(&data);
    assert!(
        result.is_err(),
        "empty tar should fail with 'manifest.json not found'"
    );
}

// -----------------------------------------------------------------------
// VerifiedBundle - construction rejects non-archives
// -----------------------------------------------------------------------

#[test]
fn test_verified_bundle_rejects_non_tar() {
    let result = bundle::VerifiedBundle::open(Arc::from(b"garbage".as_slice()), false);
    assert!(result.is_err());
}
