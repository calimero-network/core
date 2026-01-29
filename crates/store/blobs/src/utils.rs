/// Validates that a path component (e.g. package name or version) is safe to use in a filesystem path.
///
/// This measure ensures components used in path construction do not contain separators,
/// traversal sequences, or special characters.
///
/// # Allowed format
///
/// * Alphanumeric characters (`a-z`, `A-Z`, `0-9`);
/// * Hyphens (`-`);
/// * Single dots (`.`), but not at the start or end of the string.
///
/// # Arguments
///
/// * `component`: The string to validate.
/// * `component_label`: An optional description of what is being validated (e.g., "package name", "version name").
///   Used for error messaging. If `None`, the default value "component" will be used for errors.
///
/// # Panics
///
/// This function panics if the `component`:
/// * Is empty;
/// * Contains characters other than alphanumeric, hyphens, or dots (blocks `/`, `\`, etc.);
/// * Contains consecutive dots (`..`);
/// * Starts or ends with a dot.
pub fn validate_path_component(component: &str, component_label: Option<&str>) {
    let component_label = component_label.unwrap_or("component");

    // Ensure the component is not empty.
    if component.is_empty() {
        panic!("{component_label} cannot be empty");
    }

    // Prevent path traversal.
    if component.contains("..") {
        panic!("{component_label} cannot contain '..': '{component}'");
    }

    // Allow only alphanumeric characters, hyphen, and dot.
    if component
        .chars()
        .any(|c| !c.is_alphanumeric() && c != '-' && c != '.')
    {
        panic!(
            "invalid character in {component_label}: '{component}'. Only alphanumeric, '-', and '.' are allowed.",
        );
    }

    // Forbid using dots at boundaries.
    if component.starts_with('.') || component.ends_with('.') {
        panic!("{component_label} cannot start or end with '.': '{component}'");
    }
}

#[cfg(test)]
mod utils {
    use super::*;

    #[test]
    fn test_validate_path_component_success() {
        // Valid cases
        validate_path_component("calimero", None);
        validate_path_component("calimero-network", Some("package"));
        validate_path_component("v1.0.0", Some("version"));
        validate_path_component("1.2.3", None);
        validate_path_component("calimero.world-controller-app", None);
        validate_path_component("0123456789", None);
    }

    #[test]
    #[should_panic(expected = "package cannot be empty")]
    fn test_validate_path_component_empty_package() {
        validate_path_component("", Some("package"));
    }

    #[test]
    #[should_panic(expected = "version cannot be empty")]
    fn test_validate_path_component_empty_version() {
        validate_path_component("", Some("version"));
    }

    #[test]
    #[should_panic(expected = "component cannot be empty")]
    fn test_validate_path_component_empty_default() {
        validate_path_component("", None);
    }

    #[test]
    #[should_panic(expected = "invalid character in package name")]
    fn test_validate_path_component_slash() {
        validate_path_component("calimero/network", Some("package name"));
    }

    #[test]
    #[should_panic(expected = "invalid character")]
    fn test_validate_path_component_backslash() {
        validate_path_component("calimero\\network", None);
    }

    #[test]
    #[should_panic(expected = "invalid character")]
    fn test_validate_path_component_space() {
        validate_path_component("calimero network", None);
    }

    #[test]
    #[should_panic(expected = "invalid character")]
    fn test_validate_path_component_special_chars() {
        validate_path_component("calimero@network", None);
    }

    #[test]
    #[should_panic(expected = "package name cannot contain '..'")]
    fn test_validate_path_component_traversal() {
        validate_path_component("../etc", Some("package name"));
    }

    #[test]
    #[should_panic(expected = "version name cannot contain '..'")]
    fn test_validate_path_component_double_dot_middle() {
        validate_path_component("calimero..network", Some("version name"));
    }

    #[test]
    #[should_panic(expected = "version name cannot contain '..'")]
    fn test_validate_path_component_multiple_dots() {
        validate_path_component("calimero...network", Some("version name"));
    }

    #[test]
    #[should_panic(expected = "package name cannot start or end with '.'")]
    fn test_validate_path_component_start_dot() {
        validate_path_component(".calimero", Some("package name"));
    }

    #[test]
    #[should_panic(expected = "version name cannot start or end with '.'")]
    fn test_validate_path_component_end_dot() {
        validate_path_component("calimero.", Some("version name"));
    }
}
