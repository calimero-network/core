//! Integration tests for meroctl CLI commands.
//!
//! This module tests CLI command parsing, output formats, error handling,
//! and argument validation using `assert_cmd`.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

/// Helper to get a Command instance for meroctl
fn meroctl() -> Command {
    Command::cargo_bin("meroctl").expect("Failed to find meroctl binary")
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
        // app get requires an application ID argument
        meroctl()
            .args(["app", "get"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_app_install_help_shows_options() {
        // app install requires either --path or --url flag
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
        // Test that 'ls' alias works for list
        meroctl()
            .args(["blob", "ls", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Usage:"));
    }

    #[test]
    fn test_blob_delete_alias() {
        // Test that 'rm' alias works for delete
        meroctl()
            .args(["blob", "rm"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
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
        // Test that 'ls' alias works
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
        // context get has subcommands (info, client-keys, storage)
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
        // Test that 'del' alias works
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
        // Node list should work without needing a connection
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
    fn test_node_add_validates_name() {
        let temp = tempdir().expect("Failed to create temp dir");
        // Node name with invalid characters should fail
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
    fn test_node_add_allows_valid_name() {
        let temp = tempdir().expect("Failed to create temp dir");
        // This will fail because the path doesn't exist, but name validation passes
        meroctl()
            .args([
                "--home",
                temp.path().to_str().unwrap(),
                "node",
                "add",
                "valid-node_1",
                "/nonexistent/path",
            ])
            .assert()
            .failure()
            // The error should be about config not found, not about invalid name
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
    fn test_node_remove_aliases() {
        // Test that 'rm' alias works
        meroctl()
            .args(["node", "rm"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }

    #[test]
    fn test_node_add_connect_alias() {
        // Test that 'connect' alias works for add
        meroctl()
            .args(["node", "connect"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("required"));
    }
}

// =============================================================================
// Output Format Tests
// =============================================================================

mod output_format {
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
        // Test that API flag accepts URL format
        meroctl()
            .args(["--api", "http://localhost:8080", "--help"])
            .assert()
            .success();
    }

    #[test]
    fn test_api_and_node_conflict() {
        // --api and --node should conflict, must include a command to trigger validation
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
}

// =============================================================================
// Validation Tests
// =============================================================================

mod validation {
    use super::*;

    #[test]
    fn test_blob_upload_nonexistent_file() {
        // Uploading a non-existent file should fail with file not found error
        meroctl()
            .args(["blob", "upload", "--file", "/nonexistent/file.wasm"])
            .assert()
            .failure();
    }

    #[test]
    fn test_blob_upload_existing_file_validates() {
        // Create a temp file and verify it passes file validation
        let temp = tempdir().expect("Failed to create temp dir");
        let file_path = temp.path().join("test.wasm");
        fs::write(&file_path, b"test content").expect("Failed to write test file");

        // This will fail at connection time, but the file validation should pass
        meroctl()
            .args([
                "--api",
                "http://127.0.0.1:99999",
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
