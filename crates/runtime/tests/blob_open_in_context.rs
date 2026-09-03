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

/// A read-only execution must not be able to produce a delta. The executor
/// gates delta creation on a non-empty artifact and separately logs-and-discards
/// a view that mutated (`execute/mod.rs:2169-2178`); a blob read must take
/// neither branch.
#[test]
fn read_only_storage_wrapper_does_not_suppress_blob_calls() {
    let storage_src = include_str!("../../context/src/handlers/execute/storage.rs");
    // The wrapper suppresses exactly these; nothing blob-related.
    for suppressed in ["fn set(", "fn remove(", "fn index_set(", "fn index_del("] {
        assert!(
            storage_src.contains(suppressed),
            "expected read-only wrapper to define {suppressed}"
        );
    }
    assert!(
        !storage_src.contains("blob"),
        "the read-only storage wrapper must not know about blobs — \
         blob host calls go through node_client, not the Storage trait"
    );
}
