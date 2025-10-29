///! Test RGA synchronization behavior
use borsh::{BorshDeserialize, BorshSerialize};

use crate::collections::ReplicatedGrowableArray;
use crate::env;

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

    assert_eq!(text, "Hello World", "Text should be preserved after serialize/deserialize");
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

