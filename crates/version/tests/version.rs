use calimero_version::version_info;

#[test]
fn version_info_contains_crate_version() {
    let info = version_info();
    assert!(
        info.contains(env!("CARGO_PKG_VERSION")),
        "version_info does not contain crate version"
    );
}

#[test]
fn version_info_contains_commit_hash() {
    let info = version_info();
    assert!(
        info.contains(env!("GIT_COMMIT_HASH")),
        "version_info does not contain GIT_COMMIT_HASH"
    );
}

#[test]
fn version_info_contains_rustc_version() {
    let info = version_info();
    assert!(
        info.contains(env!("RUSTC_VERSION")),
        "version_info does not contain RUSTC_VERSION"
    );
}

#[test]
fn version_info_contains_protocol_version() {
    let info = version_info();
    assert!(
        info.contains(&format!("(protocol {})", env!("CARGO_PKG_VERSION_MAJOR"))),
        "version_info does not contain protocol version"
    );
}

#[test]
fn version_info_has_no_unknown_values() {
    let info = version_info();
    assert!(
        !info.contains("unknown"),
        "version_info contains 'unknown': {}",
        info
    );
}
