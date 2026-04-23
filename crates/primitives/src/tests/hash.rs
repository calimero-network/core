use serde_json::{from_value as from_json_value, json, to_string as to_json_string};

use super::*;

#[test]
fn zero_hash_matches_from_bytes() {
    let from_bytes: Hash = [0u8; 32].into();
    assert_eq!(Hash::zero(), from_bytes);
    assert_eq!(from_bytes.to_string(), "11111111111111111111111111111111");
}

#[test]
fn test_hash_43() {
    let hash = Hash::new(b"Hello, World");

    assert_eq!(
        hex::encode(hash.as_bytes()),
        "03675ac53ff9cd1535ccc7dfcdfa2c458c5218371f418dc136f2d19ac1fbe8a5"
    );

    assert_eq!(
        hash.to_string(),
        "EHdZfnzn717B56XYH8sWLAHfDC3icGEkccNzpAF4PwS"
    );
    assert_eq!(
        hash.to_base58(),
        "EHdZfnzn717B56XYH8sWLAHfDC3icGEkccNzpAF4PwS"
    );
}

#[test]
fn test_hash_44() {
    let hash = Hash::new(b"Hello World");

    assert_eq!(
        hex::encode(hash.as_bytes()),
        "a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e"
    );

    assert_eq!(
        hash.to_string(),
        "C9K5weED8iiEgM6bkU6gZSgGsV6DW2igMtNtL1sjfFKK"
    );

    assert_eq!(
        hash.to_base58(),
        "C9K5weED8iiEgM6bkU6gZSgGsV6DW2igMtNtL1sjfFKK"
    );
}

#[test]
fn encode_base58_into_stack_buf() {
    let hash = Hash::new(b"Hello World");
    let mut buf = [0u8; 45];
    let s = hash.encode_base58(&mut buf);
    assert_eq!(s, "C9K5weED8iiEgM6bkU6gZSgGsV6DW2igMtNtL1sjfFKK");
}

#[test]
fn test_serde() {
    let hash = Hash::new(b"Hello World");

    assert_eq!(
        to_json_string(&hash).unwrap(),
        "\"C9K5weED8iiEgM6bkU6gZSgGsV6DW2igMtNtL1sjfFKK\""
    );

    assert_eq!(
        from_json_value::<Hash>(json!("C9K5weED8iiEgM6bkU6gZSgGsV6DW2igMtNtL1sjfFKK")).unwrap(),
        hash
    );
}
