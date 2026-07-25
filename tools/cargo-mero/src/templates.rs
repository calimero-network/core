//! Scaffold templates, embedded at compile time.

/// Output path (relative to the new app) and template body, in write order.
pub const FILES: &[(&str, &str)] = &[
    ("Cargo.toml", include_str!("../templates/Cargo.toml.tmpl")),
    ("build.rs", include_str!("../templates/build.rs.tmpl")),
    ("src/lib.rs", include_str!("../templates/lib.rs.tmpl")),
    (
        "tests/converge.rs",
        include_str!("../templates/converge.rs.tmpl"),
    ),
    ("README.md", include_str!("../templates/README.md.tmpl")),
    (".gitignore", include_str!("../templates/gitignore.tmpl")),
];

/// The scaffold's `calimero-storage` testing dev-dep, read back out of the
/// template so a hint quoting it cannot drift from what `new` actually writes.
pub fn testing_dev_dep(sdk_version: &str) -> Option<String> {
    let cargo_toml = FILES.iter().find(|(out, _)| *out == "Cargo.toml")?.1;
    cargo_toml
        .lines()
        .find(|line| line.starts_with("calimero-storage") && line.contains("\"testing\""))
        .map(|line| line.replace(TAG_PLACEHOLDER, sdk_version))
}

pub const TAG_PLACEHOLDER: &str = "{{sdk_tag}}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn testing_dev_dep_is_found_in_the_template() {
        let line = testing_dev_dep("1.2.3").expect("template must carry a testing dev-dep line");
        assert!(line.contains("features = [\"testing\"]"));
        assert!(line.contains("1.2.3"));
        assert!(!line.contains(TAG_PLACEHOLDER));
    }
}
