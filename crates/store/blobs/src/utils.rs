/// Validates that a path component (e.g. package name or version) is safe to use in a filesystem path.
///
/// This measure ensures components used in path construction do not contain separators,
/// traversal sequences, or special characters.
///
/// # Allowed format
///
/// * ASCII alphanumeric characters (`a-z`, `A-Z`, `0-9`);
/// * Hyphens (`-`);
/// * Single dots (`.`), but not at the start or end of the string.
///
/// # Arguments
///
/// * `component`: The string to validate.
/// * `component_label`: An optional description of what is being validated (e.g., "package name", "version name").
///   Used for error messaging. If `None`, the default value "component" will be used for errors.
///
/// # Errors
///
/// Returns an error if the `component`:
/// * Is empty;
/// * Contains characters other than ASCII alphanumeric, hyphens, or dots (blocks `/`, `\`, etc.);
/// * Contains consecutive dots (`..`);
/// * Starts or ends with a dot.
pub fn validate_path_component(
    component: &str,
    component_label: Option<&str>,
) -> eyre::Result<()> {
    let component_label = component_label.unwrap_or("component");

    // Ensure the component is not empty.
    if component.is_empty() {
        eyre::bail!("{component_label} cannot be empty");
    }

    // Prevent path traversal.
    if component.contains("..") {
        eyre::bail!("{component_label} cannot contain '..': '{component}'");
    }

    // Allow only ASCII alphanumeric characters, hyphen, and dot. The check is
    // deliberately ASCII-restricted: a Unicode-aware `is_alphanumeric` would
    // admit homoglyphs and full-width digits that can confuse path handling.
    if component
        .chars()
        .any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '.')
    {
        eyre::bail!(
            "invalid character in {component_label}: '{component}'. Only ASCII alphanumeric, '-', and '.' are allowed.",
        );
    }

    // Forbid using dots at boundaries.
    if component.starts_with('.') || component.ends_with('.') {
        eyre::bail!("{component_label} cannot start or end with '.': '{component}'");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts the component is rejected and the error message contains `needle`.
    fn assert_rejected(component: &str, label: Option<&str>, needle: &str) {
        let err = validate_path_component(component, label)
            .expect_err("expected component to be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains(needle),
            "error {msg:?} should contain {needle:?}",
        );
    }

    #[test]
    fn test_validate_path_component_success() {
        // Valid cases
        validate_path_component("calimero", None).unwrap();
        validate_path_component("calimero-network", Some("package")).unwrap();
        validate_path_component("v1.0.0", Some("version")).unwrap();
        validate_path_component("1.2.3", None).unwrap();
        validate_path_component("calimero.world-controller-app", None).unwrap();
        validate_path_component("0123456789", None).unwrap();
    }

    #[test]
    fn test_validate_path_component_empty_package() {
        assert_rejected("", Some("package"), "package cannot be empty");
    }

    #[test]
    fn test_validate_path_component_empty_version() {
        assert_rejected("", Some("version"), "version cannot be empty");
    }

    #[test]
    fn test_validate_path_component_empty_default() {
        assert_rejected("", None, "component cannot be empty");
    }

    #[test]
    fn test_validate_path_component_slash() {
        assert_rejected(
            "calimero/network",
            Some("package name"),
            "invalid character in package name",
        );
    }

    #[test]
    fn test_validate_path_component_backslash() {
        assert_rejected("calimero\\network", None, "invalid character");
    }

    #[test]
    fn test_validate_path_component_space() {
        assert_rejected("calimero network", None, "invalid character");
    }

    #[test]
    fn test_validate_path_component_special_chars() {
        assert_rejected("calimero@network", None, "invalid character");
    }

    #[test]
    fn test_validate_path_component_non_ascii_alphanumeric() {
        // Unicode alphanumerics (e.g. full-width digits, accented letters) are
        // ASCII-restricted out, where the old `is_alphanumeric` check let them
        // through.
        assert_rejected("ｃalimero", None, "invalid character");
        assert_rejected("café", None, "invalid character");
    }

    #[test]
    fn test_validate_path_component_traversal() {
        assert_rejected("../etc", Some("package name"), "package name cannot contain '..'");
    }

    #[test]
    fn test_validate_path_component_double_dot_middle() {
        assert_rejected(
            "calimero..network",
            Some("version name"),
            "version name cannot contain '..'",
        );
    }

    #[test]
    fn test_validate_path_component_multiple_dots() {
        assert_rejected(
            "calimero...network",
            Some("version name"),
            "version name cannot contain '..'",
        );
    }

    #[test]
    fn test_validate_path_component_start_dot() {
        assert_rejected(
            ".calimero",
            Some("package name"),
            "package name cannot start or end with '.'",
        );
    }

    #[test]
    fn test_validate_path_component_end_dot() {
        assert_rejected(
            "calimero.",
            Some("version name"),
            "version name cannot start or end with '.'",
        );
    }
}
