#![no_main]
//! Log entries: every replica decodes each entry its group's leader
//! replicated, a component app's as a `ComponentCmd` (after reading its format
//! marker) and the system group's as a `SystemCmd`. A replica that cannot
//! decode an entry stops its group, so what one node writes every other node
//! of the same release must read back to the same entry.
//!
//! The input is one entry. Whatever decodes must encode, decode again and
//! encode to the same bytes; what this release writes carries no format
//! marker (format 1).

use calimero_node::component::{entry_format, ComponentCmd, ENTRY_FORMAT};
use calimero_node::system::SystemCmd;
use libfuzzer_sys::fuzz_target;
use serde::de::DeserializeOwned;
use serde::Serialize;

fuzz_target!(|data: &[u8]| {
    // A replica reads the marker before it decodes, and applies only what it reads.
    let _ = entry_format(data);
    if let Some(bytes) = stable::<ComponentCmd>(data) {
        assert_eq!(
            entry_format(&bytes),
            Some(ENTRY_FORMAT),
            "this release wrote an entry in a format other than its own"
        );
    }
    let _ = stable::<SystemCmd>(data);
});

/// The entry `data` decodes to, encoded: it decodes again and encodes to the same bytes.
fn stable<T: Serialize + DeserializeOwned>(data: &[u8]) -> Option<Vec<u8>> {
    let entry: T = serde_json::from_slice(data).ok()?;
    let first = serde_json::to_vec(&entry).expect("a decoded entry encodes");
    let again: T = match serde_json::from_slice(&first) {
        Ok(again) => again,
        Err(e) => panic!(
            "an entry this release wrote does not decode: {e}\n{}",
            String::from_utf8_lossy(&first)
        ),
    };
    let second = serde_json::to_vec(&again).expect("a decoded entry encodes");
    assert!(
        first == second,
        "an entry encodes differently after a round trip:\n{}\n{}",
        String::from_utf8_lossy(&first),
        String::from_utf8_lossy(&second)
    );
    Some(first)
}
