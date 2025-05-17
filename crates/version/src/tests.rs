
use crate::version_str;

#[test]
fn version_info_contains_crate_version() {
    let info = version_str();
    let expected = option_env!("CARGO_PKG_VERSION").unwrap_or("<missing>");
    assert!(
        info.contains(expected),
        "version_info does not contain crate version: expected `{}` in `{}`",
        expected,
        info
    );
}

#[test]
fn version_info_contains_commit_hash() {
    let info = version_str();
    let expected = option_env!("GIT_COMMIT").unwrap_or("<missing>");
    assert!(
        info.contains(expected),
        "version_info does not contain GIT_COMMIT: expected `{}` in `{}`",
        expected,
        info
    );
}

#[test]
fn version_info_contains_rustc_version() {
    let info = version_str();
    let expected = option_env!("RUSTC_VERSION").unwrap_or("<missing>");
    assert!(
        info.contains(expected),
        "version_info does not contain RUSTC_VERSION: expected `{}` in `{}`",
        expected,
        info
    );
}

#[test]
fn version_info_contains_protocol_version() {
    let info = version_str();
    let expected = option_env!("CARGO_PKG_VERSION_MAJOR").unwrap_or("<missing>");
    let expected_str = format!("(protocol {})", expected);
    assert!(
        info.contains(&expected_str),
        "version_info does not contain protocol version: expected `{}` in `{}`",
        expected_str,
        info
    );
}

#[test]
fn version_info_has_no_unknown_values() {
    let info = version_str();
    assert!(
        !info.contains("unknown"),
        "version_info contains 'unknown': {}",
        info
    );
}
