#![no_main]
//! TDX quotes: a client checks the quote of every node it talks to, and every
//! node the quote of every peer, from bytes that peer chose. The read must
//! refuse what it cannot parse and never panic. Built as a release is
//! (mock-attestation off), so a mock quote is just bytes too.

use calimero_tee_attestation::{quote_report_data, verify_evidence};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = quote_report_data(data);
    // Without collateral nothing verifies: a real quote needs it, and a mock
    // quote is not one.
    assert!(
        verify_evidence(data, None, 0, &[0; 32]).is_err(),
        "a quote verified without collateral"
    );
});
