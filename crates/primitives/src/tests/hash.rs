use serde_json::{from_value as from_json_value, json, to_string as to_json_string};

use super::*;

#[test]
fn test_hash_43() {
    let hash = Hash::new(b"Hello, World");

    assert_eq!(
        hex::encode(hash.as_bytes()),
        "03675ac53ff9cd1535ccc7dfcdfa2c458c5218371f418dc136f2d19ac1fbe8a5"
    );

    assert_eq!(hash.as_str(), "EHdZfnzn717B56XYH8sWLAHfDC3icGEkccNzpAF4PwS");
    assert_eq!(
        (*&*&*&*&*&*&hash).as_str(),
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
        hash.as_str(),
        "C9K5weED8iiEgM6bkU6gZSgGsV6DW2igMtNtL1sjfFKK"
    );

    assert_eq!(
        (*&*&*&*&*&*&hash).as_str(),
        "C9K5weED8iiEgM6bkU6gZSgGsV6DW2igMtNtL1sjfFKK"
    );
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
