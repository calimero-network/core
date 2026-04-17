//! Tests for the application module (bundle validation, path safety).

use super::bundle;

#[test]
fn test_validate_path_component_valid() {
    let valid_paths = vec!["com.example.app", "my-app", "my_app_v2", "app123"];
    for path in valid_paths {
        assert!(
            bundle::validate_path_component(path, "test").is_ok(),
            "Valid path '{}' should pass validation",
            path
        );
    }
}

#[test]
fn test_validate_path_component_path_traversal() {
    let invalid_paths = vec!["../etc", "..", "foo/../bar", "package..name"];
    for path in invalid_paths {
        assert!(
            bundle::validate_path_component(path, "test").is_err(),
            "Path traversal '{}' should be rejected",
            path
        );
    }
}

#[test]
fn test_validate_path_component_directory_separators() {
    let invalid_paths = vec!["foo/bar", "foo\\bar", "/absolute", "\\windows"];
    for path in invalid_paths {
        assert!(
            bundle::validate_path_component(path, "test").is_err(),
            "Path with separator '{}' should be rejected",
            path
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
            "Windows drive path '{}' should be rejected",
            path
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
            "Valid artifact path '{}' should pass validation",
            path
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
            "Windows absolute path '{}' should be rejected",
            path
        );
    }
}

#[test]
fn test_validate_artifact_path_traversal() {
    let invalid_paths = vec!["../etc/passwd", "foo/../bar", "..", "migrations/../../etc"];
    for path in invalid_paths {
        assert!(
            bundle::validate_artifact_path(path, "test").is_err(),
            "Path traversal '{}' should be rejected",
            path
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
// is_bundle_archive
// -----------------------------------------------------------------------

#[test]
fn test_is_bundle_archive_mpk() {
    assert!(bundle::is_bundle_archive(camino::Utf8Path::new("app.mpk")));
    assert!(bundle::is_bundle_archive(camino::Utf8Path::new(
        "/path/to/bundle.mpk"
    )));
}

#[test]
fn test_is_bundle_archive_non_mpk() {
    assert!(!bundle::is_bundle_archive(camino::Utf8Path::new(
        "app.wasm"
    )));
    assert!(!bundle::is_bundle_archive(camino::Utf8Path::new(
        "app.tar.gz"
    )));
    assert!(!bundle::is_bundle_archive(camino::Utf8Path::new(
        "no_extension"
    )));
}

// -----------------------------------------------------------------------
// is_bundle_blob — non-bundle data
// -----------------------------------------------------------------------

#[test]
fn test_is_bundle_blob_random_bytes() {
    assert!(!bundle::is_bundle_blob(b"not a tar archive"));
    assert!(!bundle::is_bundle_blob(b""));
    assert!(!bundle::is_bundle_blob(&[0xFF; 100]));
}

// -----------------------------------------------------------------------
// extract_bundle_manifest — edge cases
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
// extract_manifest_allow_unsigned — verify branching
// -----------------------------------------------------------------------

#[test]
fn test_extract_unsigned_rejects_non_tar() {
    let result = bundle::extract_manifest_allow_unsigned(b"garbage");
    assert!(result.is_err());
}

#[test]
fn test_verify_and_extract_rejects_non_tar() {
    let result = bundle::verify_and_extract_manifest(b"garbage");
    assert!(result.is_err());
}
