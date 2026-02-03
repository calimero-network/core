//! Integration tests for meroctl CLI commands.
//!
//! This module tests CLI command parsing, output formats, error handling,
//! argument validation, and interaction with mock servers using `assert_cmd`
//! and `wiremock`.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Helper to get a Command instance for meroctl
fn meroctl() -> Command {
    Command::cargo_bin("meroctl").expect("Failed to find meroctl binary")
}

// =============================================================================
// Mock Server Integration Tests - HTTP Error Handling
// =============================================================================

mod http_error_handling {
    use super::*;

    #[tokio::test]
    async fn test_server_error_500_returns_failure() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/applications"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "app",
                "list",
            ])
            .assert()
            .failure();
    }

    #[tokio::test]
    async fn test_server_not_found_404_returns_failure() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/applications"))
            .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "app",
                "list",
            ])
            .assert()
            .failure();
    }

    #[tokio::test]
    async fn test_unauthorized_401_returns_failure() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/applications"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "app",
                "list",
            ])
            .assert()
            .failure();
    }

    #[tokio::test]
    async fn test_forbidden_403_returns_failure() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/blobs"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "blob",
                "list",
            ])
            .assert()
            .failure();
    }

    #[tokio::test]
    async fn test_bad_request_400_returns_failure() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/contexts"))
            .respond_with(ResponseTemplate::new(400).set_body_string("Bad Request"))
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "context",
                "list",
            ])
            .assert()
            .failure();
    }

    #[tokio::test]
    async fn test_invalid_json_response_returns_failure() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/applications"))
            .respond_with(ResponseTemplate::new(200).set_body_string("this is not json"))
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "app",
                "list",
            ])
            .assert()
            .failure();
    }

    #[tokio::test]
    async fn test_malformed_json_response_returns_failure() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/applications"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"invalid\": }"))
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "app",
                "list",
            ])
            .assert()
            .failure();
    }

    #[tokio::test]
    async fn test_empty_response_body_returns_failure() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/peers"))
            .respond_with(ResponseTemplate::new(200).set_body_string(""))
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "peers",
            ])
            .assert()
            .failure();
    }

    #[test]
    fn test_connection_refused_returns_failure() {
        let temp = tempdir().expect("Failed to create temp dir");
        // Use a port that's very unlikely to be in use
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                "http://127.0.0.1:59999",
                "app",
                "list",
            ])
            .assert()
            .failure();
    }

    #[test]
    fn test_invalid_url_format() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                "not-a-valid-url",
                "app",
                "list",
            ])
            .assert()
            .failure();
    }
}

// =============================================================================
// Mock Server - Request Verification Tests
// =============================================================================

mod request_verification {
    use super::*;

    #[tokio::test]
    async fn test_app_list_sends_get_request_to_correct_endpoint() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/applications"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        let _result = meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "app",
                "list",
            ])
            .assert();

        // If we get here without a panic, the mock was matched
    }

    #[tokio::test]
    async fn test_blob_list_sends_get_request_to_correct_endpoint() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/blobs"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        let _result = meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "blob",
                "list",
            ])
            .assert();
    }

    #[tokio::test]
    async fn test_context_list_sends_get_request_to_correct_endpoint() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/contexts"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        let _result = meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "context",
                "list",
            ])
            .assert();
    }

    #[tokio::test]
    async fn test_peers_sends_get_request_to_correct_endpoint() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/peers"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        let _result = meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "peers",
            ])
            .assert();
    }

    #[tokio::test]
    async fn test_blob_delete_command_runs_without_panic() {
        let mock_server = MockServer::start().await;

        // The blob delete command will try to make a DELETE request
        // Even if it fails, it shouldn't panic
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "blob",
                "delete",
                "--blob-id",
                "test-blob-id",
            ])
            .assert()
            .failure();
    }

    #[tokio::test]
    async fn test_app_uninstall_command_runs_without_panic() {
        let mock_server = MockServer::start().await;

        // The app uninstall command will try to make a DELETE request
        // Even if it fails, it shouldn't panic
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "app",
                "uninstall",
                "test-app-id",
            ])
            .assert()
            .failure();
    }
}

// =============================================================================
// Output Format Tests
// =============================================================================

mod output_format_tests {
    use super::*;

    #[test]
    fn test_output_format_json_flag_is_valid() {
        meroctl()
            .args(["--output-format", "json", "--help"])
            .assert()
            .success();
    }

    #[test]
    fn test_output_format_human_flag_is_valid() {
        meroctl()
            .args(["--output-format", "human", "--help"])
            .assert()
            .success();
    }

    #[test]
    fn test_output_format_invalid_value() {
        meroctl()
            .args(["--output-format", "xml"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("invalid value"));
    }

    #[test]
    fn test_output_format_yaml_invalid() {
        meroctl()
            .args(["--output-format", "yaml"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("invalid value"));
    }

    #[tokio::test]
    async fn test_json_output_produces_json_on_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/applications"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        let output = meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "--output-format",
                "json",
                "app",
                "list",
            ])
            .assert()
            .failure()
            .get_output()
            .stdout
            .clone();

        // When output format is JSON, errors should also be JSON
        let stdout_str = String::from_utf8(output).expect("Invalid UTF-8");
        // The output should start with { indicating JSON
        assert!(
            stdout_str.trim().starts_with('{') || stdout_str.is_empty(),
            "Expected JSON output or empty, got: {}",
            stdout_str
        );
    }

    #[tokio::test]
    async fn test_human_output_format_on_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/admin-api/applications"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                &mock_server.uri(),
                "--output-format",
                "human",
                "app",
                "list",
            ])
            .assert()
            .failure()
            // Human format should show ERROR in a table
            .stdout(predicate::str::contains("ERROR"));
    }
}

// =============================================================================
// Basic CLI Tests (Help, Version)
// =============================================================================

mod help_and_version {
    use super::*;

    #[test]
    fn test_help_output() {
        meroctl()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"))
            .stdout(predicate::str::contains("Commands:"))
            .stdout(predicate::str::contains("app"))
            .stdout(predicate::str::contains("blob"))
            .stdout(predicate::str::contains("context"))
            .stdout(predicate::str::contains("call"))
            .stdout(predicate::str::contains("peers"))
            .stdout(predicate::str::contains("node"));
    }

    #[test]
    fn test_version_output() {
        meroctl()
            .arg("--version")
            .assert()
            .success()
            .stdout(predicate::str::contains("meroctl"));
    }

    #[test]
    fn test_help_short_flag() {
        meroctl()
            .arg("-h")
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }
}

// =============================================================================
// App Command Tests
// =============================================================================

mod app_commands {
    use super::*;

    #[test]
    fn test_app_help() {
        meroctl()
            .args(["app", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "Command for managing applications",
            ))
            .stdout(predicate::str::contains("get"))
            .stdout(predicate::str::contains("install"))
            .stdout(predicate::str::contains("list"))
            .stdout(predicate::str::contains("uninstall"))
            .stdout(predicate::str::contains("watch"));
    }

    #[test]
    fn test_app_list_help() {
        meroctl()
            .args(["app", "list", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }

    #[test]
    fn test_app_get_requires_app_id() {
        meroctl()
            .args(["app", "get"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_app_install_help_shows_options() {
        meroctl()
            .args(["app", "install", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--path"))
            .stdout(predicate::str::contains("--url"));
    }

    #[test]
    fn test_app_uninstall_requires_app_id() {
        meroctl()
            .args(["app", "uninstall"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_app_list_alias_ls() {
        meroctl()
            .args(["app", "ls", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }
}

// =============================================================================
// Blob Command Tests
// =============================================================================

mod blob_commands {
    use super::*;

    #[test]
    fn test_blob_help() {
        meroctl()
            .args(["blob", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Command for managing blobs"))
            .stdout(predicate::str::contains("list"))
            .stdout(predicate::str::contains("upload"))
            .stdout(predicate::str::contains("download"))
            .stdout(predicate::str::contains("info"))
            .stdout(predicate::str::contains("delete"));
    }

    #[test]
    fn test_blob_list_help() {
        meroctl()
            .args(["blob", "list", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }

    #[test]
    fn test_blob_upload_requires_file() {
        meroctl()
            .args(["blob", "upload"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_blob_download_requires_arguments() {
        meroctl()
            .args(["blob", "download"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_blob_info_requires_blob_id() {
        meroctl()
            .args(["blob", "info"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_blob_delete_requires_blob_id() {
        meroctl()
            .args(["blob", "delete"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_blob_list_alias() {
        meroctl()
            .args(["blob", "ls", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }

    #[test]
    fn test_blob_delete_alias() {
        meroctl()
            .args(["blob", "rm"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_blob_upload_nonexistent_file() {
        meroctl()
            .args(["blob", "upload", "--file", "/nonexistent/file.wasm"])
            .assert()
            .failure();
    }

    #[test]
    fn test_blob_upload_existing_file_validates() {
        let temp = tempdir().expect("Failed to create temp dir");
        let file_path = temp.path().join("test.wasm");
        fs::write(&file_path, b"test content").expect("Failed to write test file");

        // This will fail at connection time, but the file validation should pass
        meroctl()
            .args([
                "--api",
                "http://127.0.0.1:59999",
                "blob",
                "upload",
                "--file",
                file_path.to_str().unwrap(),
            ])
            .assert()
            .failure()
            // Should not fail because of file validation
            .stderr(predicate::str::contains("File not found").not());
    }
}

// =============================================================================
// Context Command Tests
// =============================================================================

mod context_commands {
    use super::*;

    #[test]
    fn test_context_help() {
        meroctl()
            .args(["context", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Command for managing contexts"))
            .stdout(predicate::str::contains("list"))
            .stdout(predicate::str::contains("create"))
            .stdout(predicate::str::contains("join"))
            .stdout(predicate::str::contains("invite"))
            .stdout(predicate::str::contains("get"))
            .stdout(predicate::str::contains("delete"));
    }

    #[test]
    fn test_context_list_help() {
        meroctl()
            .args(["context", "list", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }

    #[test]
    fn test_context_list_alias() {
        meroctl()
            .args(["context", "ls", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }

    #[test]
    fn test_context_create_requires_arguments() {
        meroctl()
            .args(["context", "create"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_context_join_requires_arguments() {
        meroctl()
            .args(["context", "join"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_context_get_has_subcommands() {
        meroctl()
            .args(["context", "get", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("info"))
            .stdout(predicate::str::contains("client-keys"))
            .stdout(predicate::str::contains("storage"));
    }

    #[test]
    fn test_context_delete_requires_context_id() {
        meroctl()
            .args(["context", "delete"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_context_delete_alias() {
        meroctl()
            .args(["context", "del"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_context_identity_help() {
        meroctl()
            .args(["context", "identity", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }

    #[test]
    fn test_context_proposals_help() {
        meroctl()
            .args(["context", "proposals", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }
}

// =============================================================================
// Call Command Tests
// =============================================================================

mod call_commands {
    use super::*;

    #[test]
    fn test_call_help() {
        meroctl()
            .args(["call", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Call a method on a context"))
            .stdout(predicate::str::contains("--context"))
            .stdout(predicate::str::contains("--args"))
            .stdout(predicate::str::contains("--as"));
    }

    #[test]
    fn test_call_requires_method() {
        meroctl()
            .args(["call"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_call_with_invalid_json_args() {
        meroctl()
            .args(["call", "my_method", "--args", "not-valid-json"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error"));
    }

    #[test]
    fn test_call_with_valid_json_args_parses() {
        // This should pass argument parsing but fail on connection
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                "http://127.0.0.1:59999",
                "call",
                "my_method",
                "--args",
                r#"{"key": "value"}"#,
            ])
            .assert()
            .failure()
            // Should not fail on JSON parsing
            .stderr(predicate::str::contains("invalid JSON").not());
    }

    #[test]
    fn test_call_accepts_complex_json_args() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                "http://127.0.0.1:59999",
                "call",
                "complex_method",
                "--args",
                r#"{"nested": {"array": [1, 2, 3], "bool": true}, "string": "test"}"#,
            ])
            .assert()
            .failure()
            // Should not fail on JSON parsing
            .stderr(predicate::str::contains("invalid JSON").not());
    }
}

// =============================================================================
// Peers Command Tests
// =============================================================================

mod peers_commands {
    use super::*;

    #[test]
    fn test_peers_help() {
        meroctl()
            .args(["peers", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "Return the number of connected peers",
            ));
    }
}

// =============================================================================
// Node Command Tests
// =============================================================================

mod node_commands {
    use super::*;

    #[test]
    fn test_node_help() {
        meroctl()
            .args(["node", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Command for managing nodes"))
            .stdout(predicate::str::contains("add"))
            .stdout(predicate::str::contains("remove"))
            .stdout(predicate::str::contains("use"))
            .stdout(predicate::str::contains("list"));
    }

    #[test]
    fn test_node_list_works() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args(["--home", temp.path().to_str().unwrap(), "node", "list"])
            .assert()
            .success();
    }

    #[test]
    fn test_node_list_alias() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args(["--home", temp.path().to_str().unwrap(), "node", "ls"])
            .assert()
            .success();
    }

    #[test]
    fn test_node_add_requires_arguments() {
        meroctl()
            .args(["node", "add"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_node_add_validates_name_rejects_at_symbol() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "node",
                "add",
                "node@invalid",
                "/some/path",
            ])
            .assert()
            .failure()
            .stderr(predicate::str::contains("invalid characters"));
    }

    #[test]
    fn test_node_add_validates_name_rejects_spaces() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "node",
                "add",
                "node with spaces",
                "/some/path",
            ])
            .assert()
            .failure()
            .stderr(predicate::str::contains("invalid characters"));
    }

    #[test]
    fn test_node_add_allows_valid_name_with_hyphen() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "node",
                "add",
                "valid-node",
                "/nonexistent/path",
            ])
            .assert()
            .failure()
            .stderr(predicate::str::contains("invalid characters").not());
    }

    #[test]
    fn test_node_add_allows_valid_name_with_underscore() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "node",
                "add",
                "valid_node_1",
                "/nonexistent/path",
            ])
            .assert()
            .failure()
            .stderr(predicate::str::contains("invalid characters").not());
    }

    #[test]
    fn test_node_remove_requires_name() {
        meroctl()
            .args(["node", "remove"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_node_use_requires_name() {
        meroctl()
            .args(["node", "use"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_node_use_nonexistent_fails() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "node",
                "use",
                "nonexistent-node",
            ])
            .assert()
            .failure()
            // Error message is written to stdout in table format
            .stdout(predicate::str::contains("does not exist"));
    }

    #[test]
    fn test_node_remove_alias_rm() {
        meroctl()
            .args(["node", "rm"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_node_remove_alias_disconnect() {
        meroctl()
            .args(["node", "disconnect"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_node_add_alias_connect() {
        meroctl()
            .args(["node", "connect"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }
}

// =============================================================================
// Root Args Tests
// =============================================================================

mod root_args {
    use super::*;

    #[test]
    fn test_home_flag() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args(["--home", temp.path().to_str().unwrap(), "--help"])
            .assert()
            .success();
    }

    #[test]
    fn test_api_flag_format() {
        meroctl()
            .args(["--api", "http://localhost:8080", "--help"])
            .assert()
            .success();
    }

    #[test]
    fn test_api_and_node_conflict() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "--api",
                "http://localhost:8080",
                "--node",
                "mynode",
                "node",
                "list",
            ])
            .assert()
            .failure()
            .stderr(predicate::str::contains("cannot be used with"));
    }
}

// =============================================================================
// Error Handling Tests
// =============================================================================

mod error_handling {
    use super::*;

    #[test]
    fn test_unknown_command_error() {
        meroctl()
            .args(["unknown-command"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error"));
    }

    #[test]
    fn test_unknown_subcommand_error() {
        meroctl()
            .args(["app", "unknown-subcommand"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error"));
    }

    #[test]
    fn test_invalid_flag_error() {
        meroctl()
            .args(["--invalid-flag"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error"));
    }

    #[test]
    fn test_extra_positional_args_error() {
        meroctl()
            .args(["app", "list", "extra", "args"])
            .assert()
            .failure();
    }
}

// =============================================================================
// Environment Variable Tests
// =============================================================================

mod env_vars {
    use super::*;

    #[test]
    fn test_calimero_home_env_var() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .env("CALIMERO_HOME", temp.path())
            .args(["--help"])
            .assert()
            .success();
    }

    #[test]
    fn test_calimero_home_env_var_used_for_node_list() {
        let temp = tempdir().expect("Failed to create temp dir");
        meroctl()
            .env("CALIMERO_HOME", temp.path())
            .args(["node", "list"])
            .assert()
            .success();
    }
}

// =============================================================================
// Examples from Help Tests
// =============================================================================

mod examples {
    use super::*;

    #[test]
    fn test_help_contains_examples() {
        meroctl()
            .args(["--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Examples:"));
    }

    #[test]
    fn test_app_help_contains_examples() {
        meroctl()
            .args(["app", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Examples:"));
    }

    #[test]
    fn test_context_help_contains_examples() {
        meroctl()
            .args(["context", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Examples:"));
    }

    #[test]
    fn test_node_help_contains_examples() {
        meroctl()
            .args(["node", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Examples:"));
    }

    #[test]
    fn test_blob_help_contains_examples() {
        meroctl()
            .args(["blob", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Examples:"));
    }
}
