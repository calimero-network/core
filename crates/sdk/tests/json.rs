//! 64-bit integers are decimal strings in JSON events and errors.

use calimero_sdk::json::{i64_str, opt_u64_str, u64_str};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Booked {
    #[serde(with = "u64_str")]
    start: u64,
    #[serde(with = "i64_str")]
    delta: i64,
    #[serde(default, with = "opt_u64_str")]
    end: Option<u64>,
}

#[test]
fn sixty_four_bit_integers_are_decimal_strings() {
    let v = Booked {
        start: u64::MAX,
        delta: i64::MIN,
        end: Some(7),
    };
    let json = serde_json::to_string(&v).unwrap();
    assert_eq!(
        json,
        r#"{"start":"18446744073709551615","delta":"-9223372036854775808","end":"7"}"#
    );
    assert_eq!(serde_json::from_str::<Booked>(&json).unwrap(), v);
    let none = Booked {
        start: 1,
        delta: 2,
        end: None,
    };
    assert_eq!(
        serde_json::to_string(&none).unwrap(),
        r#"{"start":"1","delta":"2","end":null}"#
    );
    assert_eq!(
        serde_json::from_str::<Booked>(r#"{"start":"1","delta":"2"}"#).unwrap(),
        none
    );
}

#[test]
fn numbers_are_not_accepted_where_a_string_is_the_contract() {
    assert!(serde_json::from_str::<Booked>(r#"{"start":1,"delta":"2"}"#).is_err());
    assert!(serde_json::from_str::<Booked>(r#"{"start":"x","delta":"2"}"#).is_err());
}

#[test]
fn decimal_strings_are_canonical() {
    for bad in [
        r#"{"start":"01","delta":"0"}"#,
        r#"{"start":"+1","delta":"0"}"#,
        r#"{"start":"1","delta":"-0"}"#,
        r#"{"start":"1","delta":"-01"}"#,
    ] {
        assert!(serde_json::from_str::<Booked>(bad).is_err(), "{bad}");
    }
    let ok: Booked = serde_json::from_str(r#"{"start":"0","delta":"-5"}"#).unwrap();
    assert_eq!((ok.start, ok.delta, ok.end), (0, -5, None));
}

#[test]
fn bytes_are_stored_as_a_byte_string_and_sent_as_base64url() {
    use calimero_sdk::collections::Cell;
    use calimero_sdk::{Bytes, MemStore};
    let store = MemStore::new();
    let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let mut plain: Cell<Vec<u8>, MemStore> = Cell::with_store("plain", store.clone());
    plain.set(&data).unwrap();
    let mut bytes: Cell<Bytes, MemStore> = Cell::with_store("bytes", store.clone());
    bytes.set(&Bytes(data.clone())).unwrap();
    let size = |name: &str| -> usize {
        let key = calimero_sdk::encoding::key::encode(&[
            calimero_sdk::KeyPart::Str(name.into()),
            calimero_sdk::KeyPart::Str("v".into()),
        ]);
        calimero_sdk::Store::get(&store, &key).unwrap().len()
    };
    assert!(size("plain") > 180_000, "a Vec<u8> is a list of numbers");
    assert!(size("bytes") < 100_010, "Bytes is one byte per byte");
    assert_eq!(bytes.get().unwrap().unwrap().0, data);
    let json = serde_json::to_string(&Bytes(vec![0xfb, 0xff])).unwrap();
    assert_eq!(json, "\"-_8\"");
    let back: Bytes = serde_json::from_str(&json).unwrap();
    assert_eq!(back.0, [0xfb, 0xff]);
}
