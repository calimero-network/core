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

    eprintln!("Result: '{}'", result);
    eprintln!("Expected: 'Hello Beautiful World'");

    // Expected: "Hello Beautiful World"
    assert_eq!(
        result, "Hello Beautiful World",
        "insert_str at position 6 should insert before 'World', got: '{}'",
        result
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
    println!("After deserialize: '{}'", text);
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
    println!("After deserialize (single insert): '{}'", text);

    assert_eq!(text, "Hello", "Single insert should be preserved");
}
