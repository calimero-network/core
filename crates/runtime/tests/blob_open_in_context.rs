//! The guest ABI must declare `blob_open_in_context`, or no application can
//! link against it — and the whole point of the host function is that app code
//! can reach it.

#[test]
fn blob_open_in_context_is_declared_in_sys() {
    let sys_src = include_str!("../../sys/src/lib.rs");
    assert!(
        sys_src.contains("fn blob_open_in_context("),
        "calimero-sys must declare blob_open_in_context"
    );
}
