//! The storage encoding: key order, deterministic CBOR.

use calimero_app_sdk::encoding::key::{self, KeyPart};
use calimero_app_sdk::encoding::value;
use calimero_app_sdk::StorageError;
use proptest::prelude::*;
use serde::Serialize;

fn part() -> impl Strategy<Value = KeyPart> {
    prop_oneof![
        Just(KeyPart::None),
        any::<bool>().prop_map(KeyPart::Bool),
        prop_oneof![any::<i128>(), -3i128..3, Just(i128::MIN), Just(i128::MAX)]
            .prop_map(KeyPart::Int),
        "[a\\x00\\u{ff}b]{0,4}".prop_map(KeyPart::Str),
        proptest::collection::vec(prop_oneof![Just(0u8), Just(1), Just(255)], 0..4)
            .prop_map(KeyPart::Bytes),
    ]
}

proptest! {
    #[test]
    fn tuple_order_matches_byte_order(
        a in proptest::collection::vec(part(), 0..=4),
        b in proptest::collection::vec(part(), 0..=4),
    ) {
        prop_assert_eq!(a.cmp(&b), key::encode(&a).cmp(&key::encode(&b)));
    }
}

#[derive(Serialize)]
struct Sample {
    b: u64,
    a: &'static str,
    c: f64,
}

#[test]
fn cbor_is_deterministic() {
    let bytes = value::to_vec(&Sample {
        b: 1,
        a: "x",
        c: 1.5,
    })
    .unwrap();
    assert_eq!(hex(&bytes), "a3616161786162016163f93e00");
}

#[test]
fn nan_is_refused() {
    assert_eq!(value::to_vec(&f64::NAN), Err(StorageError::Nan));
    assert_eq!(
        value::to_vec(&Sample {
            b: 1,
            a: "x",
            c: f64::NAN
        }),
        Err(StorageError::Nan)
    );
}

#[test]
fn integers_and_floats_take_the_shortest_form() {
    assert_eq!(hex(&value::to_vec(&1u64).unwrap()), "01");
    assert_eq!(hex(&value::to_vec(&-1i64).unwrap()), "20");
    assert_eq!(hex(&value::to_vec(&1.0f32).unwrap()), "f93c00");
    assert_eq!(hex(&value::to_vec(&1.0f64).unwrap()), "f93c00");
    assert_eq!(
        value::int_from_slice(&value::int_to_vec(-5).unwrap()).unwrap(),
        -5
    );
}

#[test]
fn map_keys_sort_by_encoded_bytes() {
    let mut m = std::collections::HashMap::new();
    m.insert("bb", 1u8);
    m.insert("a", 2);
    m.insert("ccc", 3);
    // shorter encoded keys sort first
    assert_eq!(
        hex(&value::to_vec(&m).unwrap()),
        "a3616102626262016363636303"
    );
}

#[test]
fn escaping_keeps_prefixes_apart() {
    let short = key::encode(&[KeyPart::Str("a".into())]);
    let long = key::encode(&[KeyPart::Str("a\0".into())]);
    assert!(short < long);
    assert!(!long.starts_with(&short));
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
