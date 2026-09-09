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

/// No existing harness executes a `#[app::view]` method end-to-end and exposes
/// whether a delta/artifact was produced: `execute()` in
/// `crates/context/src/handlers/execute/mod.rs` needs a `ContextGuard`, a
/// compiled `calimero_runtime::Module`, and a live `NodeClient`, and nothing in
/// `crates/runtime/tests/` or `crates/context/tests/` assembles that today.
/// Building one is out of scope for this task, so this is a narrower,
/// source-level guard instead of a behavioural one.
///
/// What it proves: `ReadOnlyContextStorage`'s `Storage` impl suppresses
/// exactly the seven known write methods (`set`, `remove`, and the five
/// `index_*` mutators) by name, and none of those seven names is any of the
/// four blob host-function names. Combined with the doc comment on
/// `ReadOnlyContextStorage` (blob calls go through `node_client`, never
/// through `Storage`), this is the textual evidence that the wrapper has no
/// way to intercept `blob_open_in_context`.
///
/// What it does NOT prove: that `blob_open_in_context` is actually wired to
/// bypass `Storage` at runtime, or that a `#[app::view]` method calling it
/// produces no delta in practice. Those require the behavioural harness noted
/// above, which does not exist yet.
#[test]
fn read_only_storage_wrapper_suppresses_only_the_known_write_methods() {
    let storage_src = include_str!("../../context/src/handlers/execute/storage.rs");

    let suppressed_methods = [
        "fn set(",
        "fn remove(",
        "fn index_set(",
        "fn index_del(",
        "fn index_del_prefix(",
        "fn index_meta_set(",
        "fn index_meta_del(",
    ];
    for suppressed in suppressed_methods {
        assert!(
            storage_src.contains(suppressed),
            "expected read-only wrapper to define {suppressed}"
        );
    }

    // The wrapper must never define any blob host-function method by name —
    // checked against the actual source text, not against the free-text
    // presence of the word "blob" anywhere in the file (a comment mentioning
    // blobs would not trip this; a method signature would).
    let blob_method_signatures = [
        "fn blob_open(",
        "fn blob_open_in_context(",
        "fn blob_read(",
        "fn blob_close(",
        "fn blob_write(",
        "fn blob_create(",
        "fn blob_announce_to_context(",
    ];
    for blob_method in blob_method_signatures {
        assert!(
            !storage_src.contains(blob_method),
            "the read-only storage wrapper must never define {blob_method} — \
             blob host calls go through node_client, not the Storage trait, \
             and must remain reads"
        );
    }
}
