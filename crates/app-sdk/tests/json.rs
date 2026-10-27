//! 64-bit integers are decimal strings in JSON events and errors (D20).

use calimero_app_sdk::json::{i64_str, opt_u64_str, u64_str};
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
