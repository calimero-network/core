use crate::CalimeroVersion;

#[test]
fn version_info_contains_crate_version() {
    let info = CalimeroVersion::current_str();
    let expected = env!("CARGO_PKG_VERSION");
    assert!(
        info.contains(expected),
        "version_info does not contain crate version: expected `{}` in `{}`",
        expected,
        info
    );
}

#[test]
fn version_info_contains_commit_hash() {
    let info = CalimeroVersion::current_str();
    let expected = env!("CALIMERO_COMMIT");
    assert!(
        info.contains(expected),
        "version_info does not contain commit hash: expected `{}` in `{}`",
        expected,
        info
    );
}

#[test]
fn version_info_contains_rustc_version() {
    let info = CalimeroVersion::current_str();
    let expected = env!("CALIMERO_RUSTC_VERSION");
    assert!(
        info.contains(expected),
        "version_info does not contain rustc version: expected `{}` in `{}`",
        expected,
        info
    );
}

#[test]
fn version_info_has_no_unknown_values() {
    let info = CalimeroVersion::current_str();
    assert!(
        !info.contains("unknown"),
        "version_info contains 'unknown': {}",
        info
    );
}
