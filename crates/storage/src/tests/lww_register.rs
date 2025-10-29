use crate::collections::LwwRegister;
use crate::env;
use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use core::num::NonZeroU128;

// Helper to create timestamps for testing
fn make_timestamp(time: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(time),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}

#[test]
fn test_lww_new_and_get() {
    env::reset_for_testing();

    let reg = LwwRegister::new("Hello".to_string());
    assert_eq!(reg.get(), "Hello");
    assert_eq!(*reg, "Hello"); // Test Deref
}

#[test]
fn test_lww_set() {
    env::reset_for_testing();

    let mut reg = LwwRegister::new("Initial".to_string());
    assert_eq!(reg.get(), "Initial");

    reg.set("Updated".to_string());
    assert_eq!(reg.get(), "Updated");
}

#[test]
fn test_lww_merge_later_timestamp_wins() {
    env::reset_for_testing();

    let ts1 = make_timestamp(100);
    let ts2 = make_timestamp(200);

    let reg1 = LwwRegister::new_with_metadata("Alice".to_string(), ts1, [1u8; 32]);
    let reg2 = LwwRegister::new_with_metadata("Bob".to_string(), ts2, [2u8; 32]);

    let mut merged = reg1.clone();
    merged.merge(&reg2);

    // reg2 has later timestamp, so Bob wins
    assert_eq!(merged.get(), "Bob");
    assert_eq!(merged.timestamp(), ts2);
    assert_eq!(merged.node_id(), [2u8; 32]);
}

#[test]
fn test_lww_merge_earlier_timestamp_loses() {
    env::reset_for_testing();

    let ts1 = make_timestamp(200);
    let ts2 = make_timestamp(100);

    let reg1 = LwwRegister::new_with_metadata("Alice".to_string(), ts1, [1u8; 32]);
    let reg2 = LwwRegister::new_with_metadata("Bob".to_string(), ts2, [2u8; 32]);

    let mut merged = reg1.clone();
    merged.merge(&reg2);

    // reg1 has later timestamp, so Alice keeps
    assert_eq!(merged.get(), "Alice");
    assert_eq!(merged.timestamp(), ts1);
    assert_eq!(merged.node_id(), [1u8; 32]);
}

#[test]
fn test_lww_merge_tie_breaking_by_node_id() {
    env::reset_for_testing();

    let same_ts = make_timestamp(100);

    let reg1 = LwwRegister::new_with_metadata("Alice".to_string(), same_ts, [1u8; 32]);
    let reg2 = LwwRegister::new_with_metadata("Bob".to_string(), same_ts, [2u8; 32]);

    let mut merged = reg1.clone();
    merged.merge(&reg2);

    // Same timestamp, but node_id [2] > [1], so Bob wins
    assert_eq!(merged.get(), "Bob");
    assert_eq!(merged.timestamp(), same_ts);
    assert_eq!(merged.node_id(), [2u8; 32]);
}

#[test]
fn test_lww_merge_identical_no_change() {
    env::reset_for_testing();

    let ts = make_timestamp(100);
    let reg1 = LwwRegister::new_with_metadata("Alice".to_string(), ts, [1u8; 32]);
    let reg2 = reg1.clone();

    let mut merged = reg1.clone();
    merged.merge(&reg2);

    // Identical, no change
    assert_eq!(merged.get(), "Alice");
    assert_eq!(merged.timestamp(), ts);
}

#[test]
fn test_lww_would_update() {
    env::reset_for_testing();

    let ts1 = make_timestamp(100);
    let ts2 = make_timestamp(200);

    let reg1 = LwwRegister::new_with_metadata("Alice".to_string(), ts1, [1u8; 32]);
    let reg2 = LwwRegister::new_with_metadata("Bob".to_string(), ts2, [2u8; 32]);

    // reg2 has later timestamp, would update reg1
    assert!(reg1.would_update(&reg2));
    assert!(!reg2.would_update(&reg1));
}

#[test]
fn test_lww_concurrent_updates_converge() {
    env::reset_for_testing();

    // Simulate two nodes updating concurrently
    let node_a_ts = make_timestamp(100);
    let node_b_ts = make_timestamp(101);

    let reg_a = LwwRegister::new_with_metadata("Node A value".to_string(), node_a_ts, [1u8; 32]);
    let reg_b = LwwRegister::new_with_metadata("Node B value".to_string(), node_b_ts, [2u8; 32]);

    // Merge in both directions
    let mut a_merged_b = reg_a.clone();
    a_merged_b.merge(&reg_b);

    let mut b_merged_a = reg_b.clone();
    b_merged_a.merge(&reg_a);

    // Both should converge to the same value
    assert_eq!(a_merged_b.get(), b_merged_a.get());
    assert_eq!(a_merged_b.timestamp(), b_merged_a.timestamp());
    assert_eq!(a_merged_b.node_id(), b_merged_a.node_id());
}

#[test]
fn test_lww_three_way_merge() {
    env::reset_for_testing();

    let ts1 = make_timestamp(100);
    let ts2 = make_timestamp(200);
    let ts3 = make_timestamp(150);

    let reg1 = LwwRegister::new_with_metadata("Alice".to_string(), ts1, [1u8; 32]);
    let reg2 = LwwRegister::new_with_metadata("Bob".to_string(), ts2, [2u8; 32]);
    let reg3 = LwwRegister::new_with_metadata("Charlie".to_string(), ts3, [3u8; 32]);

    // Merge all three
    let mut result = reg1;
    result.merge(&reg2);
    result.merge(&reg3);

    // reg2 has latest timestamp (200), so Bob wins
    assert_eq!(result.get(), "Bob");
}

#[test]
fn test_lww_with_different_types() {
    env::reset_for_testing();

    // Test with u64
    let mut num = LwwRegister::new(42u64);
    num.set(100);
    assert_eq!(*num, 100);

    // Test with bool
    let mut flag = LwwRegister::new(false);
    flag.set(true);
    assert_eq!(*flag, true);

    // Test with Vec
    let mut vec = LwwRegister::new(vec![1, 2, 3]);
    vec.set(vec![4, 5, 6]);
    assert_eq!(*vec, vec![4, 5, 6]);
}

#[test]
fn test_lww_default() {
    env::reset_for_testing();

    let reg: LwwRegister<String> = LwwRegister::default();
    assert_eq!(reg.get(), "");

    let num: LwwRegister<u64> = LwwRegister::default();
    assert_eq!(*num, 0);
}

#[test]
fn test_lww_into_inner() {
    env::reset_for_testing();

    let reg = LwwRegister::new("Hello".to_string());
    let value = reg.into_inner();
    assert_eq!(value, "Hello");
}

#[test]
fn test_lww_display() {
    env::reset_for_testing();

    let reg = LwwRegister::new("Display Test".to_string());
    assert_eq!(format!("{}", reg), "Display Test");
}

#[test]
fn test_lww_sequential_updates() {
    env::reset_for_testing();

    let mut reg = LwwRegister::new("v1".to_string());
    let ts1 = reg.timestamp();

    // Small delay to ensure different timestamp
    std::thread::sleep(std::time::Duration::from_millis(1));

    reg.set("v2".to_string());
    let ts2 = reg.timestamp();

    std::thread::sleep(std::time::Duration::from_millis(1));

    reg.set("v3".to_string());
    let ts3 = reg.timestamp();

    // Timestamps should be increasing
    assert!(ts2 > ts1);
    assert!(ts3 > ts2);
    assert_eq!(reg.get(), "v3");
}

#[test]
fn test_lww_serialization() {
    env::reset_for_testing();

    let reg = LwwRegister::new("Test".to_string());

    // Serialize
    let bytes = borsh::to_vec(&reg).unwrap();

    // Deserialize
    let deserialized: LwwRegister<String> = borsh::from_slice(&bytes).unwrap();

    assert_eq!(deserialized.get(), reg.get());
    assert_eq!(deserialized.timestamp(), reg.timestamp());
    assert_eq!(deserialized.node_id(), reg.node_id());
}

#[test]
fn test_lww_merge_after_serialization() {
    env::reset_for_testing();

    let ts1 = make_timestamp(100);
    let ts2 = make_timestamp(200);

    let reg1 = LwwRegister::new_with_metadata("Alice".to_string(), ts1, [1u8; 32]);
    let reg2 = LwwRegister::new_with_metadata("Bob".to_string(), ts2, [2u8; 32]);

    // Serialize both
    let bytes1 = borsh::to_vec(&reg1).unwrap();
    let bytes2 = borsh::to_vec(&reg2).unwrap();

    // Deserialize
    let mut reg1_deserialized: LwwRegister<String> = borsh::from_slice(&bytes1).unwrap();
    let reg2_deserialized: LwwRegister<String> = borsh::from_slice(&bytes2).unwrap();

    // Merge
    reg1_deserialized.merge(&reg2_deserialized);

    // Should get Bob's value (later timestamp)
    assert_eq!(reg1_deserialized.get(), "Bob");
}
