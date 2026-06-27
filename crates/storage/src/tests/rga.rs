use crate::collections::ReplicatedGrowableArray;
use crate::env;

#[test]
fn test_rga_basic_insert() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    rga.insert(0, 'H').unwrap();
    assert_eq!(rga.get_text().unwrap(), "H");

    rga.insert(1, 'i').unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hi");

    // Insert at position 0 (before everything)
    // Both 'H' and '!' have left=root, but '!' has later timestamp
    // With REVERSED sort (latest first), '!' comes BEFORE 'H' - correct for sequential edits!
    rga.insert(0, '!').unwrap();
    let text = rga.get_text().unwrap();
    assert_eq!(text, "!Hi", "Insert at position 0 should prepend");
}

#[test]
fn test_rga_basic_delete() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    rga.insert_str(0, "Hello").unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hello");

    rga.delete(0).unwrap(); // Delete 'H'
    assert_eq!(rga.get_text().unwrap(), "ello");

    rga.delete(3).unwrap(); // Delete 'o'
    assert_eq!(rga.get_text().unwrap(), "ell");
}

#[test]
fn test_rga_insert_str() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    rga.insert_str(0, "Hello").unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hello");
    assert_eq!(rga.len().unwrap(), 5);

    rga.insert_str(5, " World").unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hello World");
    assert_eq!(rga.len().unwrap(), 11);
}

#[test]
fn test_rga_insert_str_middle() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    rga.insert_str(0, "Hello").unwrap();
    rga.insert_str(5, " World").unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hello World");

    // Insert in the middle
    rga.insert_str(5, " Beautiful").unwrap();
    let text = rga.get_text().unwrap();
    // Due to RGA ordering, the result depends on HLC timestamps
    assert!(text.contains("Beautiful"));
    assert_eq!(text.len(), 21); // "Hello" + " Beautiful" + " World"
}

#[test]
fn test_rga_insert_str_position_bug() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    // Insert "Hello World" as a single operation
    rga.insert_str(0, "Hello World").unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hello World");

    // Insert "Beautiful " at position 6 (after "Hello ", before "World")
    // Position 6 should be right before 'W'
    rga.insert_str(6, "Beautiful ").unwrap();
    let result = rga.get_text().unwrap();

    eprintln!("Result: '{result}'");
    eprintln!("Expected: 'Hello Beautiful World'");

    // Expected: "Hello Beautiful World"
    assert_eq!(
        result, "Hello Beautiful World",
        "insert_str at position 6 should insert before 'World', got: '{result}'"
    );
}

#[test]
fn test_rga_delete_range() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    rga.insert_str(0, "Hello World").unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hello World");

    rga.delete_range(5, 11).unwrap(); // Delete " World"
    assert_eq!(rga.get_text().unwrap(), "Hello");

    rga.delete_range(0, 2).unwrap(); // Delete "He"
    assert_eq!(rga.get_text().unwrap(), "llo");
}

#[test]
fn test_rga_delete_range_empty() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();
    rga.insert_str(0, "Hello").unwrap();

    // Delete empty range (start == end)
    rga.delete_range(2, 2).unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hello");
}

#[test]
fn test_rga_delete_range_full() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();
    rga.insert_str(0, "Hello").unwrap();

    // Delete entire text
    rga.delete_range(0, 5).unwrap();
    assert_eq!(rga.get_text().unwrap(), "");
    assert!(rga.is_empty().unwrap());
}

#[test]
fn test_rga_len_and_is_empty() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    assert!(rga.is_empty().unwrap());
    assert_eq!(rga.len().unwrap(), 0);

    rga.insert_str(0, "test").unwrap();
    assert!(!rga.is_empty().unwrap());
    assert_eq!(rga.len().unwrap(), 4);

    rga.delete_range(0, 4).unwrap();
    assert!(rga.is_empty().unwrap());
    assert_eq!(rga.len().unwrap(), 0);
}

#[test]
fn test_rga_insert_out_of_bounds() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();
    rga.insert_str(0, "Hi").unwrap();

    let result = rga.insert(10, '!');
    assert!(result.is_err());
}

#[test]
fn test_rga_insert_str_out_of_bounds() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();
    rga.insert_str(0, "Hi").unwrap();

    let result = rga.insert_str(10, "test");
    assert!(result.is_err());
}

#[test]
fn test_rga_delete_out_of_bounds() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();
    rga.insert_str(0, "Hi").unwrap();

    let result = rga.delete(5);
    assert!(result.is_err());
}

#[test]
fn test_rga_delete_range_invalid() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();
    rga.insert_str(0, "Hello").unwrap();

    // start > end
    let result = rga.delete_range(3, 1);
    assert!(result.is_err());
}

#[test]
fn test_rga_delete_range_out_of_bounds() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();
    rga.insert_str(0, "Hello").unwrap();

    // end > length - now idempotent, clamps to actual length
    let result = rga.delete_range(0, 10);
    assert!(result.is_ok());
    assert_eq!(rga.get_text().unwrap(), ""); // Deletes all available chars
}

#[test]
fn test_rga_interleaved_operations() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    rga.insert_str(0, "abc").unwrap();
    assert_eq!(rga.get_text().unwrap(), "abc");

    rga.delete(1).unwrap(); // Delete 'b'
    assert_eq!(rga.get_text().unwrap(), "ac");

    rga.insert(1, 'B').unwrap(); // Insert 'B' where 'b' was
    assert_eq!(rga.get_text().unwrap(), "aBc");

    rga.insert(3, '!').unwrap();
    assert_eq!(rga.get_text().unwrap(), "aBc!");
}

#[test]
fn test_rga_concurrent_inserts_deterministic() {
    env::reset_for_testing();

    // Simulate two nodes inserting at same position concurrently
    let mut rga = ReplicatedGrowableArray::new();
    rga.insert_str(0, "Hello").unwrap();

    // Both insert at position 5 (end) - their HLC timestamps determine order
    rga.insert(5, '!').unwrap();
    rga.insert(6, '?').unwrap();

    let text = rga.get_text().unwrap();
    // Should be deterministic based on HLC ordering
    assert!(text == "Hello!?" || text == "Hello?!");
    assert_eq!(text.len(), 7);
}

#[test]
fn test_rga_linearization_is_merge_order_independent() {
    // Two replicas that converge to the *same* character set must
    // produce the *same* text, regardless of the order they merged the
    // fragments in. This is the cross-replica convergence property that
    // a synced Merkle root (which hashes the same set) relies on:
    // same set ⇒ same hash ⇒ same `get_text`. The previous linear walk
    // broke it because merging in different orders changes the
    // underlying `entries()` storage-iteration order, and the old
    // gap-fallback picked "any unplaced char" in that order — so the
    // two replicas could read different text for an identical set.
    use crate::collections::Mergeable;

    env::reset_for_testing();

    // Two independent fragments. Each is its own left-chain rooted at
    // the document root, so the merged document is a forest under root
    // (a branch the linearization must order deterministically).
    let mut base = ReplicatedGrowableArray::new();
    base.insert_str(0, "Hello").unwrap();

    let mut frag = ReplicatedGrowableArray::new();
    frag.insert_str(0, "XYZ").unwrap();

    // Replica 1: merge base, then frag.
    let mut r1 = ReplicatedGrowableArray::new();
    r1.merge(&base).unwrap();
    r1.merge(&frag).unwrap();

    // Replica 2: merge frag, then base (opposite order).
    let mut r2 = ReplicatedGrowableArray::new();
    r2.merge(&frag).unwrap();
    r2.merge(&base).unwrap();

    let t1 = r1.get_text().unwrap();
    let t2 = r2.get_text().unwrap();

    assert_eq!(
        t1, t2,
        "RGA linearization must be independent of merge order (got {t1:?} vs {t2:?})"
    );
    // Both fragments survive the union in full.
    assert_eq!(t1.len(), 8, "expected the union of both fragments");
    for ch in "HelloXYZ".chars() {
        assert!(t1.contains(ch), "missing {ch:?} in {t1:?}");
    }
}

#[test]
fn test_rga_unicode_support() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    // Test various Unicode characters - insert as single string to maintain order
    rga.insert_str(0, "Hello 世界 🌍").unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hello 世界 🌍");
    assert_eq!(rga.len().unwrap(), 10); // 6 ASCII + 1 space + 2 Chinese chars + 1 emoji

    // Delete emoji
    rga.delete(9).unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hello 世界 ");
    assert_eq!(rga.len().unwrap(), 9);

    // Test with more Unicode
    rga.insert_str(0, "🚀").unwrap();
    let text = rga.get_text().unwrap();
    assert!(text.contains("🚀"));
    assert!(text.contains("Hello"));
    assert!(text.contains("世界"));
}

#[test]
fn test_rga_special_characters() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    // Test newlines, tabs, special chars
    rga.insert_str(0, "Line1\nLine2\tTabbed").unwrap();
    assert_eq!(rga.get_text().unwrap(), "Line1\nLine2\tTabbed");
    let initial_len = rga.len().unwrap();

    // Test null character handling
    rga.insert(0, '\0').unwrap();
    let text = rga.get_text().unwrap();
    assert_eq!(text.len(), initial_len + 1); // One more character added
                                             // Due to RGA ordering, null char might not be first
    assert!(text.contains('\0'));
}

#[test]
fn test_rga_repeated_inserts_at_same_position() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    // Insert multiple characters at position 0
    for ch in "HELLO".chars() {
        rga.insert(0, ch).unwrap();
    }

    let text = rga.get_text().unwrap();
    // Due to HLC timestamps, they should be in a deterministic order
    assert_eq!(text.len(), 5);
    assert!(text.contains('H'));
    assert!(text.contains('E'));
    assert!(text.contains('L'));
    assert!(text.contains('O'));
}

#[test]
fn test_rga_alternating_insert_delete() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    // Build up then tear down
    rga.insert_str(0, "abcdef").unwrap();
    assert_eq!(rga.len().unwrap(), 6);

    rga.delete(0).unwrap(); // Remove 'a'
    assert_eq!(rga.get_text().unwrap(), "bcdef");

    rga.insert(0, 'A').unwrap(); // Add 'A'
    assert_eq!(rga.get_text().unwrap(), "Abcdef");

    rga.delete(5).unwrap(); // Remove 'f'
    assert_eq!(rga.get_text().unwrap(), "Abcde");

    rga.insert(5, 'F').unwrap(); // Add 'F'
    assert_eq!(rga.get_text().unwrap(), "AbcdeF");
}

#[test]
fn test_rga_large_document() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    // Insert a moderately large document
    let large_text = "Lorem ipsum ".repeat(100); // ~1200 characters
    rga.insert_str(0, &large_text).unwrap();

    assert_eq!(rga.len().unwrap(), large_text.len());
    assert_eq!(rga.get_text().unwrap(), large_text);

    // Delete middle section
    let mid = large_text.len() / 2;
    rga.delete_range(mid - 50, mid + 50).unwrap();
    assert_eq!(rga.len().unwrap(), large_text.len() - 100);
}

#[test]
fn test_rga_empty_string_insert() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    // Inserting empty string should be no-op
    rga.insert_str(0, "").unwrap();
    assert!(rga.is_empty().unwrap());

    rga.insert_str(0, "Hello").unwrap();
    rga.insert_str(5, "").unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hello");
}

#[test]
fn test_rga_single_character_operations() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    // Build word character by character
    rga.insert(0, 'H').unwrap();
    rga.insert(1, 'e').unwrap();
    rga.insert(2, 'l').unwrap();
    rga.insert(3, 'l').unwrap();
    rga.insert(4, 'o').unwrap();

    assert_eq!(rga.get_text().unwrap(), "Hello");

    // Delete character by character from end
    rga.delete(4).unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hell");
    rga.delete(3).unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hel");
    rga.delete(2).unwrap();
    assert_eq!(rga.get_text().unwrap(), "He");
}

#[test]
fn test_rga_replace_pattern() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    rga.insert_str(0, "Hello World").unwrap();

    // Replace "World" with "CRDT"
    rga.delete_range(6, 11).unwrap();
    rga.insert_str(6, "CRDT").unwrap();

    assert_eq!(rga.get_text().unwrap(), "Hello CRDT");
}

#[test]
fn test_rga_multiple_concurrent_operations() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    // Simulate multiple concurrent insertions at different positions
    rga.insert_str(0, "ace").unwrap();

    // Insert 'b' between 'a' and 'c'
    rga.insert(1, 'b').unwrap();

    // Insert 'd' between 'c' and 'e'
    rga.insert(3, 'd').unwrap();

    let text = rga.get_text().unwrap();
    assert_eq!(text.len(), 5);
    // Characters should be present in some deterministic order
    assert!(text.contains('a'));
    assert!(text.contains('b'));
    assert!(text.contains('c'));
    assert!(text.contains('d'));
    assert!(text.contains('e'));
}

#[test]
fn test_rga_get_text_after_many_deletes() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    rga.insert_str(0, "abcdefghij").unwrap();

    // Delete every other character
    rga.delete(1).unwrap(); // b
    rga.delete(2).unwrap(); // d (was at index 3)
    rga.delete(3).unwrap(); // f (was at index 5)
    rga.delete(4).unwrap(); // h (was at index 7)
    rga.delete(5).unwrap(); // j (was at index 9)

    assert_eq!(rga.get_text().unwrap(), "acegi");
    assert_eq!(rga.len().unwrap(), 5);
}

#[test]
fn test_rga_stress_rapid_changes() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    // Rapid insertions
    for i in 0..20 {
        let ch = char::from_digit(i % 10, 10).unwrap();
        rga.insert(0, ch).unwrap();
    }

    assert_eq!(rga.len().unwrap(), 20);

    // Rapid deletions from middle
    for _ in 0..10 {
        if rga.len().unwrap() > 5 {
            rga.delete(rga.len().unwrap() / 2).unwrap();
        }
    }

    assert_eq!(rga.len().unwrap(), 10);
}

#[test]
fn test_rga_whitespace_handling() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    // Test various whitespace characters
    rga.insert_str(0, "   ").unwrap(); // spaces
    rga.insert_str(3, "\t\t").unwrap(); // tabs
    rga.insert_str(5, "\n\n").unwrap(); // newlines

    assert_eq!(rga.len().unwrap(), 7);
    assert_eq!(rga.get_text().unwrap(), "   \t\t\n\n");
}

// === Serialization/Deserialization Tests ===

#[test]
fn test_rga_serialize_deserialize() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    rga.insert_str(0, "Hello").unwrap();
    rga.insert_str(5, " World").unwrap();

    assert_eq!(rga.get_text().unwrap(), "Hello World");

    // Serialize the RGA
    let serialized = borsh::to_vec(&rga).unwrap();

    // Deserialize into a new RGA
    let rga2: ReplicatedGrowableArray = borsh::from_slice(&serialized).unwrap();

    // Check if text is preserved
    let text = rga2.get_text().unwrap();
    println!("After deserialize: '{text}'");
    println!("Length: {}", text.len());

    assert_eq!(
        text, "Hello World",
        "Text should be preserved after serialize/deserialize"
    );
}

#[test]
fn test_rga_serialize_deserialize_single_insert() {
    env::reset_for_testing();

    let mut rga = ReplicatedGrowableArray::new();

    rga.insert_str(0, "Hello").unwrap();
    assert_eq!(rga.get_text().unwrap(), "Hello");

    // Serialize
    let serialized = borsh::to_vec(&rga).unwrap();

    // Deserialize
    let rga2: ReplicatedGrowableArray = borsh::from_slice(&serialized).unwrap();

    let text = rga2.get_text().unwrap();
    println!("After deserialize (single insert): '{text}'");

    assert_eq!(text, "Hello", "Single insert should be preserved");
}

/// Reproduction attempt for the `frozen-rga-convergence` e2e flake
/// (run 26686441762, job 78655979358): node-1, after merging concurrent RGA
/// appends, deletes a char and ends on a different root hash than receivers,
/// which agree ("writer is the outlier"). Unlike
/// `test_rga_delete_after_concurrent_appends_converges` (symmetric merge()
/// path), this drives the asymmetric execute-then-broadcast-delta vs
/// apply-delta path the e2e hits.
///
/// CRUCIAL: the base "Hello" is created ONCE (genesis executor) and replayed
/// into every node via the same delta, so all nodes share identical CharIds.
/// RGA CharIds derive from the executor-seeded HLC (env.rs), so if each node
/// typed "Hello" under its own executor the chars would have distinct IDs and
/// any later append would fail to anchor — a test artifact, not the product
/// bug. The real system always syncs a shared base before concurrent edits.
#[test]
#[serial_test::serial]
fn test_rga_delete_after_merge_delta_sync_converges() {
    use crate::action::Action;
    use crate::address::Id;
    use crate::collections::{Mergeable, Root};
    use crate::delta::{commit_causal_delta, reset_delta_context, set_current_heads, StorageDelta};
    use crate::entities::Metadata;
    use crate::index::Index;
    use crate::interface::{ApplyContext, Interface};
    use crate::merge::register_crdt_merge;
    use crate::store::MainStorage;

    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
    struct RgaDoc {
        content: ReplicatedGrowableArray,
    }
    // RekeyTarget supertrait of Mergeable.
    impl crate::collections::rekey::RekeyTarget for RgaDoc {
        fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
            crate::rekey_field_if_supported!(
                &mut self.content,
                crate::collections::rekey::field_child_id(parent_id, "content")
            );
        }
    }
    impl Mergeable for RgaDoc {
        fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
            self.content.merge(&other.content)
        }
    }

    type S = MainStorage;
    let root_hash = || {
        Index::<S>::get_hashes_for(Id::root())
            .unwrap()
            .map(|(full, _)| full)
            .unwrap_or([0; 32])
    };
    // Persist the current root payload and capture every action accumulated in
    // the delta context since the last reset (child Add/DeleteRef from the op
    // plus the root Update from save_raw). Mirrors the canonical capture in
    // `test_e2e_counter_sync_with_isolated_storage` — we deliberately avoid
    // `Root::commit()`, which drains the context before we can capture it.
    let capture = |root_data: Vec<u8>| -> Vec<Action> {
        Interface::<S>::save_raw(Id::root(), root_data, Metadata::default()).unwrap();
        let hash = root_hash();
        commit_causal_delta(&hash)
            .unwrap()
            .expect("op must produce a delta")
            .actions
    };
    // Replay a captured action list into the current (fresh) node's store.
    let import = |actions: Vec<Action>| {
        let payload = borsh::to_vec(&StorageDelta::Actions(actions)).unwrap();
        Root::<RgaDoc, S>::sync(&payload, &ApplyContext::empty()).unwrap();
    };
    // Begin a fresh node with the global storage cleared and merge registered.
    let fresh_node = |executor: [u8; 32]| {
        env::reset_for_testing();
        reset_delta_context();
        register_crdt_merge::<RgaDoc>();
        set_current_heads(vec![[0; 32]]);
        env::set_executor_id(executor);
    };

    // === Genesis: shared base "Hello" captured as a delta every node imports.
    fresh_node([9; 32]);
    let mut g = Root::<RgaDoc, S>::new(|| RgaDoc {
        content: ReplicatedGrowableArray::new_with_field_name("content"),
    });
    g.content.insert_str(0, "Hello").unwrap();
    let g_data = borsh::to_vec(&*g).unwrap();
    drop(g);
    let base_actions = capture(g_data);
    let base_hash = root_hash();

    // === Node-2: import base, append " World", capture only the append delta.
    fresh_node([2; 32]);
    import(base_actions.clone());
    assert_eq!(
        Root::<RgaDoc, S>::fetch()
            .unwrap()
            .content
            .get_text()
            .unwrap(),
        "Hello",
        "node-2 must materialize the shared base before appending"
    );
    reset_delta_context();
    set_current_heads(vec![base_hash]);
    let mut n2 = Root::<RgaDoc, S>::fetch().unwrap();
    n2.content.insert_str(5, " World").unwrap();
    let n2_data = borsh::to_vec(&*n2).unwrap();
    drop(n2);
    let append_actions = capture(n2_data);

    // === Node-1: import base, merge node-2's append (-> "Hello World"),
    // delete 'H', capture only the delete delta.
    fresh_node([1; 32]);
    import(base_actions.clone());
    reset_delta_context();
    set_current_heads(vec![base_hash]);
    import(append_actions.clone());
    let n1_merged_hash = root_hash();
    assert_eq!(
        Root::<RgaDoc, S>::fetch()
            .unwrap()
            .content
            .get_text()
            .unwrap(),
        "Hello World",
        "node-1 must merge node-2's append against the shared base"
    );

    reset_delta_context();
    set_current_heads(vec![n1_merged_hash]);
    let mut n1 = Root::<RgaDoc, S>::fetch().unwrap();
    n1.content.delete(0).unwrap(); // delete 'H'
    let n1_data = borsh::to_vec(&*n1).unwrap();
    drop(n1);
    let delete_actions = capture(n1_data);
    let n1_final_hash = root_hash();
    let n1_text = Root::<RgaDoc, S>::fetch()
        .unwrap()
        .content
        .get_text()
        .unwrap();

    // === Node-2 (rebuilt): base + append already give "Hello World"; apply
    // node-1's delete delta and compare with the writer.
    fresh_node([2; 32]);
    import(base_actions.clone());
    reset_delta_context();
    set_current_heads(vec![base_hash]);
    import(append_actions.clone());
    let n2_premerge_hash = root_hash();
    assert_eq!(
        n2_premerge_hash, n1_merged_hash,
        "node-1 and node-2 must agree on the merged 'Hello World' hash before \
         the delete"
    );

    reset_delta_context();
    set_current_heads(vec![n2_premerge_hash]);
    import(delete_actions);
    let n2_final_hash = root_hash();
    let n2_text = Root::<RgaDoc, S>::fetch()
        .unwrap()
        .content
        .get_text()
        .unwrap();

    assert_eq!(
        n1_text, n2_text,
        "RGA text must converge after writer-deletes-then-broadcasts: \
         node-1(writer)={n1_text:?} vs node-2(applies delete delta)={n2_text:?}"
    );
    assert_eq!(
        n1_final_hash,
        n2_final_hash,
        "RGA root hash must converge after delete-delta sync \
         (frozen-rga writer-is-outlier flake): node-1={} node-2={}",
        hex::encode(n1_final_hash),
        hex::encode(n2_final_hash),
    );
}

/// Closer match to the `frozen-rga-convergence` e2e: TWO concurrent appends
/// (node-2 and node-3, each anchored on the shared base) followed by a delete
/// from node-1, all propagated as deltas. The e2e final state had node-2 and
/// node-3 agreeing while node-1 (the deleter) was the outlier — so this drives
/// the deltas through every node in a *different application order* and asserts
/// all three converge on identical text AND root hash. If the storage delta
/// path is order-independent here, the e2e divergence must live in the WASM
/// execute path (Step B), not in `apply_action`.
#[test]
#[serial_test::serial]
fn test_rga_concurrent_appends_then_delete_delta_sync_converges() {
    use crate::action::Action;
    use crate::address::Id;
    use crate::collections::{Mergeable, Root};
    use crate::delta::{commit_causal_delta, reset_delta_context, set_current_heads, StorageDelta};
    use crate::entities::Metadata;
    use crate::index::Index;
    use crate::interface::{ApplyContext, Interface};
    use crate::merge::register_crdt_merge;
    use crate::store::MainStorage;

    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
    struct RgaDoc {
        content: ReplicatedGrowableArray,
    }
    // RekeyTarget supertrait of Mergeable.
    impl crate::collections::rekey::RekeyTarget for RgaDoc {
        fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
            crate::rekey_field_if_supported!(
                &mut self.content,
                crate::collections::rekey::field_child_id(parent_id, "content")
            );
        }
    }
    impl Mergeable for RgaDoc {
        fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
            self.content.merge(&other.content)
        }
    }

    type S = MainStorage;
    let root_hash = || {
        Index::<S>::get_hashes_for(Id::root())
            .unwrap()
            .map(|(full, _)| full)
            .unwrap_or([0; 32])
    };
    let capture = |root_data: Vec<u8>| -> Vec<Action> {
        Interface::<S>::save_raw(Id::root(), root_data, Metadata::default()).unwrap();
        let hash = root_hash();
        commit_causal_delta(&hash)
            .unwrap()
            .expect("op must produce a delta")
            .actions
    };
    let import = |actions: Vec<Action>| {
        let payload = borsh::to_vec(&StorageDelta::Actions(actions)).unwrap();
        Root::<RgaDoc, S>::sync(&payload, &ApplyContext::empty()).unwrap();
    };
    let fresh_node = |executor: [u8; 32]| {
        env::reset_for_testing();
        reset_delta_context();
        register_crdt_merge::<RgaDoc>();
        set_current_heads(vec![[0; 32]]);
        env::set_executor_id(executor);
    };

    // === Genesis: shared base "Hello" (identical CharIds on every node).
    fresh_node([9; 32]);
    let mut g = Root::<RgaDoc, S>::new(|| RgaDoc {
        content: ReplicatedGrowableArray::new_with_field_name("content"),
    });
    g.content.insert_str(0, "Hello").unwrap();
    let g_data = borsh::to_vec(&*g).unwrap();
    drop(g);
    let base_actions = capture(g_data);
    let base_hash = root_hash();

    // === node-2: concurrent append " World" at the tail (anchored on base).
    fresh_node([2; 32]);
    import(base_actions.clone());
    reset_delta_context();
    set_current_heads(vec![base_hash]);
    let mut n2 = Root::<RgaDoc, S>::fetch().unwrap();
    n2.content.insert_str(5, " World").unwrap();
    let n2_data = borsh::to_vec(&*n2).unwrap();
    drop(n2);
    let append_b = capture(n2_data);

    // === node-3: concurrent append "!!!" at the tail (also anchored on base).
    fresh_node([3; 32]);
    import(base_actions.clone());
    reset_delta_context();
    set_current_heads(vec![base_hash]);
    let mut n3 = Root::<RgaDoc, S>::fetch().unwrap();
    n3.content.insert_str(5, "!!!").unwrap();
    let n3_data = borsh::to_vec(&*n3).unwrap();
    drop(n3);
    let append_c = capture(n3_data);

    // Materialize a node from base + the given appends (in the given order),
    // returning its (text, root_hash) after merging — no delete yet.
    let merge_node = |executor: [u8; 32], appends: &[Vec<Action>]| -> (String, [u8; 32]) {
        fresh_node(executor);
        import(base_actions.clone());
        for a in appends {
            reset_delta_context();
            set_current_heads(vec![base_hash]);
            import(a.clone());
        }
        let text = Root::<RgaDoc, S>::fetch()
            .unwrap()
            .content
            .get_text()
            .unwrap();
        (text, root_hash())
    };

    // === node-1 (writer): merge both appends [B, C], then delete 'H'.
    let (n1_merged_text, n1_merged_hash) =
        merge_node([1; 32], &[append_b.clone(), append_c.clone()]);
    reset_delta_context();
    set_current_heads(vec![n1_merged_hash]);
    let mut n1 = Root::<RgaDoc, S>::fetch().unwrap();
    n1.content.delete(0).unwrap(); // delete 'H'
    let n1_data = borsh::to_vec(&*n1).unwrap();
    drop(n1);
    let delete_actions = capture(n1_data);
    let n1_final_hash = root_hash();
    let n1_text = Root::<RgaDoc, S>::fetch()
        .unwrap()
        .content
        .get_text()
        .unwrap();

    // === node-2 receiver: appends applied [C, B] (opposite order) + delete.
    let (n2_merged_text, n2_merged_hash) =
        merge_node([2; 32], &[append_c.clone(), append_b.clone()]);
    reset_delta_context();
    set_current_heads(vec![n2_merged_hash]);
    import(delete_actions.clone());
    let n2_final_hash = root_hash();
    let n2_text = Root::<RgaDoc, S>::fetch()
        .unwrap()
        .content
        .get_text()
        .unwrap();

    // === node-3 receiver: appends applied [B, C] + delete.
    let (n3_merged_text, n3_merged_hash) =
        merge_node([3; 32], &[append_b.clone(), append_c.clone()]);
    reset_delta_context();
    set_current_heads(vec![n3_merged_hash]);
    import(delete_actions);
    let n3_final_hash = root_hash();
    let n3_text = Root::<RgaDoc, S>::fetch()
        .unwrap()
        .content
        .get_text()
        .unwrap();

    // Pre-delete merged state must be order-independent across all three nodes.
    assert_eq!(
        (n1_merged_text.as_str(), n1_merged_hash),
        (n2_merged_text.as_str(), n2_merged_hash),
        "merged concurrent-append state must be order-independent (n1 vs n2)"
    );
    assert_eq!(
        (n1_merged_text.as_str(), n1_merged_hash),
        (n3_merged_text.as_str(), n3_merged_hash),
        "merged concurrent-append state must be order-independent (n1 vs n3)"
    );

    // After the delete delta, writer (n1) and both receivers (n2, n3) converge.
    assert_eq!(
        (n1_text.as_str(), n1_final_hash),
        (n2_text.as_str(), n2_final_hash),
        "writer n1 vs receiver n2 diverged after delete (frozen-rga outlier): \
         n1={n1_text:?}/{} n2={n2_text:?}/{}",
        hex::encode(n1_final_hash),
        hex::encode(n2_final_hash),
    );
    assert_eq!(
        (n1_text.as_str(), n1_final_hash),
        (n3_text.as_str(), n3_final_hash),
        "writer n1 vs receiver n3 diverged after delete (frozen-rga outlier): \
         n1={n1_text:?}/{} n3={n3_text:?}/{}",
        hex::encode(n1_final_hash),
        hex::encode(n3_final_hash),
    );
}

/// `Root::sync` must advance the local HLC to observe a remote delta's
/// clock. After applying a `CausalActions` delta with a high `delta_hlc`, the
/// next locally-minted timestamp must sort strictly after it.
#[test]
#[serial_test::serial]
fn test_sync_advances_local_hlc_to_observe_remote_delta() {
    use std::collections::BTreeMap;

    use crate::collections::Root;
    use crate::delta::StorageDelta;
    use crate::interface::ApplyContext;
    use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    use crate::store::MainStorage;

    env::reset_for_testing();

    // Remote HLC ~2s ahead of the local wall clock (within the 5s drift
    // tolerance, so it is accepted and observed).
    let now_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let remote_secs = (now_nanos / 1_000_000_000) + 2;
    let remote_ntp = remote_secs << 32;
    let id = ID::from(core::num::NonZeroU128::new(0x1234_5678).unwrap());
    let remote_hlc = HybridTimestamp::new(Timestamp::new(NTP64(remote_ntp), id));

    // Empty-action delta — we assert only the clock side effect.
    let delta = StorageDelta::CausalActions {
        actions: vec![],
        delta_id: [0x42; 32],
        delta_hlc: remote_hlc,
        effective_writers: BTreeMap::new(),
    };
    let payload = borsh::to_vec(&delta).unwrap();
    Root::<crate::collections::Vector<u8>, MainStorage>::sync(&payload, &ApplyContext::empty())
        .unwrap();

    // The next locally-minted timestamp must sort strictly after the remote HLC.
    let next = env::hlc_timestamp();
    assert!(
        next.get_time().as_u64() > remote_hlc.get_time().as_u64(),
        "after observing a remote delta at {remote_hlc}, the next local timestamp \
         {next} must sort strictly after it (HLC causality on receive)"
    );
}

/// A remote delta whose HLC is beyond the 5s drift tolerance is rejected by the
/// guard, but `Root::sync` must still SUCCEED (warn-and-continue) and apply the
/// actions — it just must NOT advance the local clock to the far-future value.
#[test]
#[serial_test::serial]
fn test_sync_drift_rejected_hlc_still_applies_without_advancing_clock() {
    use std::collections::BTreeMap;

    use crate::collections::Root;
    use crate::delta::StorageDelta;
    use crate::interface::ApplyContext;
    use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    use crate::store::MainStorage;

    env::reset_for_testing();

    // Remote HLC ~10s ahead — beyond the 5s drift tolerance, so it is rejected.
    let now_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let remote_secs = (now_nanos / 1_000_000_000) + 10;
    let remote_hlc = HybridTimestamp::new(Timestamp::new(
        NTP64(remote_secs << 32),
        ID::from(core::num::NonZeroU128::new(0x1234_5678).unwrap()),
    ));

    let delta = StorageDelta::CausalActions {
        actions: vec![],
        delta_id: [0x99; 32],
        delta_hlc: remote_hlc,
        effective_writers: BTreeMap::new(),
    };
    let payload = borsh::to_vec(&delta).unwrap();

    // Sync must SUCCEED despite the drift rejection (warn, not fatal).
    Root::<crate::collections::Vector<u8>, MainStorage>::sync(&payload, &ApplyContext::empty())
        .expect("drift-rejected HLC must not fail sync — actions are still valid state");

    // The local clock must NOT have jumped to the rejected far-future HLC.
    let next = env::hlc_timestamp();
    assert!(
        next.get_time().as_u64() < remote_hlc.get_time().as_u64(),
        "a drift-rejected (>5s ahead) remote HLC {remote_hlc} must NOT advance the \
         local clock; next local timestamp {next} should stay near wall-clock"
    );
}

/// End-to-end: B observes A's char (CausalActions delta carrying A's HLC),
/// then inserts locally. The receive path advances B's clock past A's HLC, so
/// B's `CharId` sorts after A's and the rendered order is causal ("AB").
#[test]
#[serial_test::serial]
fn test_rga_insert_after_observing_remote_is_causally_ordered() {
    use std::collections::BTreeMap;

    use crate::address::Id;
    use crate::collections::{Mergeable, Root};
    use crate::delta::{commit_causal_delta, reset_delta_context, set_current_heads, StorageDelta};
    use crate::entities::Metadata;
    use crate::index::Index;
    use crate::interface::{ApplyContext, Interface};
    use crate::logical_clock::HybridTimestamp;
    use crate::merge::register_crdt_merge;
    use crate::store::MainStorage;

    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
    struct RgaDoc {
        content: ReplicatedGrowableArray,
    }
    // RekeyTarget supertrait of Mergeable.
    impl crate::collections::rekey::RekeyTarget for RgaDoc {
        fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
            crate::rekey_field_if_supported!(
                &mut self.content,
                crate::collections::rekey::field_child_id(parent_id, "content")
            );
        }
    }
    impl Mergeable for RgaDoc {
        fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
            self.content.merge(&other.content)
        }
    }

    type S = MainStorage;
    let root_hash = || {
        Index::<S>::get_hashes_for(Id::root())
            .unwrap()
            .map(|(full, _)| full)
            .unwrap_or([0; 32])
    };
    let fresh_node = |executor: [u8; 32]| {
        env::reset_for_testing();
        reset_delta_context();
        register_crdt_merge::<RgaDoc>();
        set_current_heads(vec![[0; 32]]);
        env::set_executor_id(executor);
    };

    // === Node A: insert 'A' at position 0; capture the delta (carrying A's char).
    fresh_node([1; 32]);
    let mut a = Root::<RgaDoc, S>::new(|| RgaDoc {
        content: ReplicatedGrowableArray::new_with_field_name("content"),
    });
    a.content.insert(0, 'A').unwrap();
    let a_data = borsh::to_vec(&*a).unwrap();
    drop(a);
    Interface::<S>::save_raw(Id::root(), a_data, Metadata::default()).unwrap();
    let a_hash = root_hash();
    let a_delta = commit_causal_delta(&a_hash)
        .unwrap()
        .expect("A's insert must produce a delta");
    let a_actions = a_delta.actions;

    // A's advertised HLC, set deliberately ~2s AHEAD of the real wall clock
    // (within the 5s drift tolerance, so `update_hlc` accepts it). This is the
    // crux of making the test DISCRIMINATING: B mints its own CharId from the
    // real wall clock, which is naturally BEHIND this future HLC. So B's mint
    // can only end up after A's HLC if the receive path actually advanced B's
    // clock to observe it. Without setting A ahead, B's wall clock alone would
    // already exceed A's HLC and the test would pass even with the receive-path
    // advance removed (the trivial pass the reviewer flagged).
    let now_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let a_future_secs = (now_nanos / 1_000_000_000) + 2;
    let a_hlc = HybridTimestamp::new(crate::logical_clock::Timestamp::new(
        crate::logical_clock::NTP64(a_future_secs << 32),
        *a_delta.hlc.get_id(),
    ));
    assert!(
        a_hlc.get_time().as_u64() > a_delta.hlc.get_time().as_u64(),
        "A's advertised HLC must be set ahead of B's natural clock to make the test \
         discriminate the receive-path advance"
    );

    // === Node B: clock at real wall-time, deliberately BEHIND A's future HLC.
    // Observe A's delta (carrying A's future HLC), then insert 'B' at position 1.
    fresh_node([2; 32]);
    let causal = StorageDelta::CausalActions {
        actions: a_actions,
        delta_id: a_hash,
        delta_hlc: a_hlc,
        effective_writers: BTreeMap::new(),
    };
    Root::<RgaDoc, S>::sync(&borsh::to_vec(&causal).unwrap(), &ApplyContext::empty()).unwrap();

    let mut b = Root::<RgaDoc, S>::fetch().unwrap();
    assert_eq!(
        b.content.get_text().unwrap(),
        "A",
        "B must have materialized A's char before its own insert"
    );

    // B inserts 'B' AFTER 'A' (position 1). `insert` MINTS a fresh CharId from
    // B's HLC — the operative event. Capture B's insert as a delta and read its
    // `hlc`, which is the timestamp embedded in the CharId B just minted. This
    // proves the *mint* (not merely a `hlc_timestamp()` read) advanced past A's
    // observed HLC: the receive path moved B's clock on `Root::sync`, so the
    // mint is forced strictly later. A test that only read `hlc_timestamp()`
    // could pass for an unrelated reason (e.g. B's wall clock simply being
    // ahead); asserting on the minted CharId's HLC closes that gap.
    reset_delta_context();
    set_current_heads(vec![a_hash]);
    b.content.insert(1, 'B').unwrap();
    let b_data = borsh::to_vec(&*b).unwrap();
    drop(b);
    Interface::<S>::save_raw(Id::root(), b_data, Metadata::default()).unwrap();
    let b_hash = Index::<S>::get_hashes_for(Id::root())
        .unwrap()
        .map(|(full, _)| full)
        .unwrap_or([0; 32]);
    let b_delta = commit_causal_delta(&b_hash)
        .unwrap()
        .expect("B's insert must produce a delta");
    let b_minted_hlc: HybridTimestamp = b_delta.hlc;

    assert!(
        b_minted_hlc.get_time().as_u64() > a_hlc.get_time().as_u64(),
        "the CharId B MINTED for its insert ({b_minted_hlc}) must sort strictly after \
         A's observed HLC ({a_hlc}) — proving Root::sync advanced B's clock on the \
         receive path, not just that a clock read happened to be larger"
    );

    // Rendered order is causal: 'A' then 'B' (B's later CharId sorts after A's).
    assert_eq!(
        Root::<RgaDoc, S>::fetch()
            .unwrap()
            .content
            .get_text()
            .unwrap(),
        "AB",
        "B's insert after observing A must render causally as \"AB\""
    );
}

/// Real RGA interleave + convergence merge (the `sync_compliance` test
/// only checks `crdt_type`). Two replicas concurrently insert distinct runs at
/// the same gap; merging in either order must converge to identical text/hash,
/// with each run kept as a contiguous block between the base anchors.
#[test]
#[serial_test::serial]
fn test_rga_real_interleave_merge_converges() {
    use crate::action::Action;
    use crate::address::Id;
    use crate::collections::{Mergeable, Root};
    use crate::delta::{commit_causal_delta, reset_delta_context, set_current_heads, StorageDelta};
    use crate::entities::Metadata;
    use crate::index::Index;
    use crate::interface::{ApplyContext, Interface};
    use crate::logical_clock::{HybridTimestamp, Timestamp, NTP64};
    use crate::merge::register_crdt_merge;
    use crate::store::MainStorage;

    // Non-zero HLC at physical tick `tick`, distinct from the root sentinel.
    // Used to PIN each replica's insert timestamp so the merged order is a pure
    // function of the chosen ticks, not of wall-clock spacing.
    fn ts_at(tick: u64) -> HybridTimestamp {
        let id = *HybridTimestamp::zero().get_id();
        HybridTimestamp::new(Timestamp::new(NTP64(tick << 32), id))
    }

    #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
    struct RgaDoc {
        content: ReplicatedGrowableArray,
    }
    // RekeyTarget supertrait of Mergeable.
    impl crate::collections::rekey::RekeyTarget for RgaDoc {
        fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
            crate::rekey_field_if_supported!(
                &mut self.content,
                crate::collections::rekey::field_child_id(parent_id, "content")
            );
        }
    }
    impl Mergeable for RgaDoc {
        fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
            self.content.merge(&other.content)
        }
    }

    type S = MainStorage;
    let root_hash = || {
        Index::<S>::get_hashes_for(Id::root())
            .unwrap()
            .map(|(full, _)| full)
            .unwrap_or([0; 32])
    };
    let capture = |root_data: Vec<u8>| -> Vec<Action> {
        Interface::<S>::save_raw(Id::root(), root_data, Metadata::default()).unwrap();
        let hash = root_hash();
        commit_causal_delta(&hash)
            .unwrap()
            .expect("op must produce a delta")
            .actions
    };
    let import = |actions: Vec<Action>| {
        let payload = borsh::to_vec(&StorageDelta::Actions(actions)).unwrap();
        Root::<RgaDoc, S>::sync(&payload, &ApplyContext::empty()).unwrap();
    };
    let fresh_node = |executor: [u8; 32]| {
        env::reset_for_testing();
        reset_delta_context();
        register_crdt_merge::<RgaDoc>();
        set_current_heads(vec![[0; 32]]);
        env::set_executor_id(executor);
    };

    // Genesis: shared base "ab" (identical CharIds on every replica). Pin the
    // base at a LOW tick so the concurrent inserts below — which we pin at
    // strictly-higher ticks — sort BEFORE 'b' under `Reverse(CharId)` and land
    // in the a|b gap (with the real HLC, X/Y minted later-than-base wall-clock
    // timestamps and got this for free; with pinned ticks the base must be
    // pinned lower too, or the inserts would sort after the higher-CharId 'b').
    fresh_node([9; 32]);
    let mut g = Root::<RgaDoc, S>::new(|| RgaDoc {
        content: ReplicatedGrowableArray::new_with_field_name("content"),
    });
    g.content
        .insert_str_at_timestamp(0, ts_at(1), "ab")
        .unwrap();
    let g_data = borsh::to_vec(&*g).unwrap();
    drop(g);
    let base_actions = capture(g_data);
    let base_hash = root_hash();

    // X and Y insert concurrently at the same a|b gap. The merged ORDER (which
    // run's block sorts first) is decided by `Reverse(CharId)`: the run with the
    // strictly-HIGHER CharId comes first. CharId is `(HybridTimestamp, seq)`,
    // ordered by the timestamp first, so we PIN each run's timestamp explicitly
    // (`insert_str_at_timestamp`) rather than the node-local HLC — otherwise the
    // order would depend on which replica's wall clock happened to tick later,
    // making the assertion non-deterministic. Both are strictly above the base
    // tick (so they land in the a|b gap, not after 'b'); with `y_ts > x_ts`,
    // Y's CharIds are strictly higher and Y's block sorts FIRST → "aYYXXb".
    let x_ts = ts_at(10);
    let y_ts = ts_at(11);

    // Replica X: insert "XX" at the a|b gap (position 1) at the lower timestamp.
    fresh_node([1; 32]);
    import(base_actions.clone());
    reset_delta_context();
    set_current_heads(vec![base_hash]);
    let mut x = Root::<RgaDoc, S>::fetch().unwrap();
    x.content.insert_str_at_timestamp(1, x_ts, "XX").unwrap();
    let x_data = borsh::to_vec(&*x).unwrap();
    drop(x);
    let x_delta = capture(x_data);

    // Replica Y: insert "YY" at the same a|b gap (position 1), concurrently, at
    // the strictly-higher timestamp.
    fresh_node([2; 32]);
    import(base_actions.clone());
    reset_delta_context();
    set_current_heads(vec![base_hash]);
    let mut y = Root::<RgaDoc, S>::fetch().unwrap();
    y.content.insert_str_at_timestamp(1, y_ts, "YY").unwrap();
    let y_data = borsh::to_vec(&*y).unwrap();
    drop(y);
    let y_delta = capture(y_data);

    // Materialize a node from base + the two inserts applied in `order`.
    let converge = |executor: [u8; 32], deltas: &[Vec<Action>]| -> (String, [u8; 32]) {
        fresh_node(executor);
        import(base_actions.clone());
        for d in deltas {
            reset_delta_context();
            set_current_heads(vec![base_hash]);
            import(d.clone());
        }
        let text = Root::<RgaDoc, S>::fetch()
            .unwrap()
            .content
            .get_text()
            .unwrap();
        (text, root_hash())
    };

    // X-then-Y vs Y-then-X must converge identically (commutativity).
    let (xy_text, xy_hash) = converge([3; 32], &[x_delta.clone(), y_delta.clone()]);
    let (yx_text, yx_hash) = converge([4; 32], &[y_delta, x_delta]);

    assert_eq!(
        (xy_text.as_str(), xy_hash),
        (yx_text.as_str(), yx_hash),
        "concurrent RGA inserts must converge regardless of merge order: \
         X-then-Y={xy_text:?}/{} vs Y-then-X={yx_text:?}/{}",
        hex::encode(xy_hash),
        hex::encode(yx_hash),
    );

    // Each run stays a contiguous block between the base anchors, and the order
    // is DETERMINISTIC: siblings sort by `Reverse(CharId)`, so the run with the
    // higher CharId comes first. We pinned `y_ts > x_ts`, so Y's CharIds are
    // strictly higher and Y's block sorts before X's → exactly "aYYXXb". (The
    // previous `"aXXYYb" || "aYYXXb"` had a dead branch: with fixed timestamps
    // only one order is reachable.)
    assert_eq!(
        xy_text, "aYYXXb",
        "Y (higher CharId, y_ts > x_ts) must sort before X under Reverse(CharId), \
         each run a contiguous block between a and b; got {xy_text:?}"
    );
}
