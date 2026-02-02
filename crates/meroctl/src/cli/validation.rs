//! Input validation utilities for CLI commands.
//!
//! This module provides reusable validation functions for common CLI inputs
//! like file paths, strings, URLs, and identifiers.

use std::path::Path;

use eyre::bail;
use eyre::Result;

/// Validates that a string is non-empty.
///
/// # Arguments
/// * `value` - The string to validate
/// * `field_name` - Name of the field for error messages
///
/// # Errors
/// Returns an error if the string is empty or contains only whitespace.
pub fn validate_non_empty(value: &str, field_name: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{} cannot be empty", field_name);
    }
    Ok(())
}

/// Validates that a file exists and is readable.
///
/// # Arguments
/// * `path` - The file path to validate
///
/// # Errors
/// Returns an error if the file doesn't exist or is not a file.
pub fn validate_file_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("File not found: '{}'", path.display());
    }
    if !path.is_file() {
        bail!("Path is not a file: '{}'", path.display());
    }
    Ok(())
}

/// Validates that a directory exists.
///
/// # Arguments
/// * `path` - The directory path to validate
///
/// # Errors
/// Returns an error if the directory doesn't exist or is not a directory.
#[allow(dead_code)]
pub fn validate_directory_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("Directory not found: '{}'", path.display());
    }
    if !path.is_dir() {
        bail!("Path is not a directory: '{}'", path.display());
    }
    Ok(())
}

/// Validates that a parent directory exists and is a directory.
///
/// # Arguments
/// * `path` - The file path whose parent directory should be validated
///
/// # Errors
/// Returns an error if the parent directory doesn't exist or isn't a directory.
pub fn validate_parent_directory_exists(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };

    if parent.as_os_str().is_empty() {
        return Ok(());
    }

    if !parent.exists() {
        bail!("Parent directory does not exist: '{}'", parent.display());
    }

    if !parent.is_dir() {
        bail!("Parent path is not a directory: '{}'", parent.display());
    }

    Ok(())
}

/// Validates that a URL string is well-formed.
///
/// # Arguments
/// * `url_str` - The URL string to validate
///
/// # Errors
/// Returns an error if the URL is malformed.
#[allow(dead_code)]
pub fn validate_url(url_str: &str) -> Result<url::Url> {
    url::Url::parse(url_str).map_err(|e| {
        eyre::eyre!(
            "Invalid URL '{}': {}. Expected format: http(s)://hostname[:port][/path]",
            url_str,
            e
        )
    })
}

/// Validates a node name.
///
/// Node names must be non-empty and contain only alphanumeric characters,
/// hyphens, and underscores.
///
/// # Arguments
/// * `name` - The node name to validate
///
/// # Errors
/// Returns an error if the node name is invalid.
#[allow(dead_code)]
pub fn validate_node_name(name: &str) -> Result<()> {
    validate_non_empty(name, "Node name")?;

    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        bail!(
            "Node name '{}' contains invalid characters. Only alphanumeric characters, hyphens (-), and underscores (_) are allowed",
            name
        );
    }

    Ok(())
}

/// Clap value parser for non-empty strings.
///
/// Can be used with `#[arg(value_parser = non_empty_string)]`
pub fn non_empty_string(s: &str) -> Result<String, String> {
    if s.trim().is_empty() {
        Err("value cannot be empty".to_string())
    } else {
        Ok(s.to_string())
    }
}

/// Clap value parser for existing file paths.
///
/// Can be used with `#[arg(value_parser = existing_file_path)]`
pub fn existing_file_path(s: &str) -> Result<std::path::PathBuf, String> {
    let path = std::path::PathBuf::from(s);
    if !path.exists() {
        return Err(format!("File not found: '{}'", s));
    }
    if !path.is_file() {
        return Err(format!("Path is not a file: '{}'", s));
    }
    Ok(path)
}

/// Clap value parser for valid node names.
///
/// Can be used with `#[arg(value_parser = valid_node_name)]`
pub fn valid_node_name(s: &str) -> Result<String, String> {
    if s.trim().is_empty() {
        return Err("Node name cannot be empty".to_string());
    }

    if !s
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "Node name '{}' contains invalid characters. Only alphanumeric characters, hyphens (-), and underscores (_) are allowed",
            s
        ));
    }

    Ok(s.to_string())
}

/// Clap value parser for valid URLs.
///
/// Can be used with `#[arg(value_parser = valid_url)]`
pub fn valid_url(s: &str) -> Result<String, String> {
    match url::Url::parse(s) {
        Ok(_) => Ok(s.to_string()),
        Err(e) => Err(format!(
            "Invalid URL '{}': {}. Expected format: http(s)://hostname[:port][/path]",
            s, e
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::tempdir;
    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn test_validate_non_empty_valid() {
        assert!(validate_non_empty("test", "field").is_ok());
        assert!(validate_non_empty("  test  ", "field").is_ok());
    }

    #[test]
    fn test_validate_non_empty_invalid() {
        assert!(validate_non_empty("", "field").is_err());
        assert!(validate_non_empty("   ", "field").is_err());
        assert!(validate_non_empty("\t\n", "field").is_err());
    }

    #[test]
    fn test_validate_file_exists_valid() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "test content").unwrap();
        assert!(validate_file_exists(temp_file.path()).is_ok());
    }

    #[test]
    fn test_validate_file_exists_not_found() {
        let path = Path::new("/nonexistent/file/path.txt");
        let result = validate_file_exists(path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("File not found"));
    }

    #[test]
    fn test_validate_directory_exists_valid() {
        let temp_dir = tempdir().unwrap();
        assert!(validate_directory_exists(temp_dir.path()).is_ok());
    }

    #[test]
    fn test_validate_directory_exists_not_found() {
        let path = Path::new("/nonexistent/directory/path");
        let result = validate_directory_exists(path);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Directory not found"));
    }

    #[test]
    fn test_validate_directory_exists_not_directory() {
        let temp_file = NamedTempFile::new().unwrap();
        let result = validate_directory_exists(temp_file.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Path is not a directory"));
    }

    #[test]
    fn test_validate_parent_directory_exists_valid() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("output.txt");
        assert!(validate_parent_directory_exists(&path).is_ok());
    }

    #[test]
    fn test_validate_parent_directory_exists_missing() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("missing").join("output.txt");
        let result = validate_parent_directory_exists(&path);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Parent directory does not exist"));
    }

    #[test]
    fn test_validate_parent_directory_exists_not_directory() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().join("output.txt");
        let result = validate_parent_directory_exists(&path);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Parent path is not a directory"));
    }

    #[test]
    fn test_validate_url_valid() {
        assert!(validate_url("http://localhost:8080").is_ok());
        assert!(validate_url("https://example.com/path").is_ok());
        assert!(validate_url("http://127.0.0.1:2528").is_ok());
    }

    #[test]
    fn test_validate_url_invalid() {
        let result = validate_url("not-a-url");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid URL"));
    }

    #[test]
    fn test_validate_node_name_valid() {
        assert!(validate_node_name("node1").is_ok());
        assert!(validate_node_name("my-node").is_ok());
        assert!(validate_node_name("my_node_123").is_ok());
        assert!(validate_node_name("Node-1").is_ok());
    }

    #[test]
    fn test_validate_node_name_empty() {
        assert!(validate_node_name("").is_err());
        assert!(validate_node_name("   ").is_err());
    }

    #[test]
    fn test_validate_node_name_invalid_chars() {
        let result = validate_node_name("node@1");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid characters"));

        assert!(validate_node_name("node.name").is_err());
        assert!(validate_node_name("node name").is_err());
    }

    #[test]
    fn test_non_empty_string_parser() {
        assert!(non_empty_string("test").is_ok());
        assert!(non_empty_string("").is_err());
        assert!(non_empty_string("   ").is_err());
    }

    #[test]
    fn test_valid_node_name_parser() {
        assert!(valid_node_name("node1").is_ok());
        assert!(valid_node_name("").is_err());
        assert!(valid_node_name("node@1").is_err());
    }

    #[test]
    fn test_valid_url_parser() {
        assert!(valid_url("http://localhost:8080").is_ok());
        assert!(valid_url("not-a-url").is_err());
    }
}
