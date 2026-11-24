#![no_main]
//! What a leader and a job node send each other: a job node decodes an
//! `Assignment` from the body of `POST /job/runs` and checks the cluster's
//! signature over it, and a leader decodes a job node's `Report` and checks
//! its signature (`verify_report`), both from bytes the other side chose.
//!
//! The input is split at 0xff into two documents, each read as an assignment
//! and as a report. Whatever decodes is signed with a key the target holds:
//! it must verify under that key only, still verify after it is encoded and
//! decoded again, and two documents that differ must never sign the same
//! bytes (a signature over one would hold for the other). A signature the
//! fuzzer wrote never verifies.

use calimero_node::job_wire::{
    assignment_message, report_message, sign_assignment, sign_report, verify_assignment,
    verify_report, Assignment, Report,
};
use ed25519_dalek::SigningKey;
use libfuzzer_sys::fuzz_target;

const KEY: [u8; 32] = [0x4b; 32];
const OTHER: [u8; 32] = [0x4c; 32];
/// The digest of the assignment a report answers.
const DIGEST: [u8; 32] = [0x5d; 32];

fuzz_target!(|data: &[u8]| {
    let key = SigningKey::from_bytes(&KEY);
    let public = key.verifying_key().to_bytes();
    let other = SigningKey::from_bytes(&OTHER).verifying_key().to_bytes();

    let mut parts = data.splitn(2, |b| *b == 0xff);
    let docs = [parts.next().unwrap_or(&[]), parts.next().unwrap_or(&[])];

    let assignments: Vec<Assignment> = docs
        .iter()
        .filter_map(|d| serde_json::from_slice(d).ok())
        .collect();
    for a in &assignments {
        assignment(a, &key, &public, &other);
    }
    if let [a, b] = assignments.as_slice() {
        if unsigned(a) != unsigned(b) {
            if let (Ok(ma), Ok(mb)) = (assignment_message(a), assignment_message(b)) {
                assert_ne!(ma, mb, "two different assignments sign the same bytes");
            }
        }
    }

    let reports: Vec<(Report, &[u8])> = docs
        .iter()
        .filter_map(|d| Some((serde_json::from_slice(d).ok()?, *d)))
        .collect();
    for (r, _) in &reports {
        report(r, &key, &public, &other);
    }
    if let [(a, _), (b, _)] = reports.as_slice() {
        if a != b {
            if let (Ok(ma), Ok(mb)) = (report_message(a, &DIGEST), report_message(b, &DIGEST)) {
                assert_ne!(ma, mb, "two different reports sign the same bytes");
            }
        }
    }
});

fn unsigned(a: &Assignment) -> Assignment {
    Assignment {
        signature: String::new(),
        ..a.clone()
    }
}

fn assignment(a: &Assignment, key: &SigningKey, public: &[u8; 32], other: &[u8; 32]) {
    assert!(
        !verify_assignment(public, a),
        "an assignment the fuzzer signed verified"
    );
    let Ok(signature) = sign_assignment(key, a) else {
        assert!(assignment_message(a).is_err());
        return;
    };
    let signed = Assignment {
        signature,
        ..a.clone()
    };
    assert!(
        verify_assignment(public, &signed),
        "a signed assignment does not verify"
    );
    assert!(
        !verify_assignment(other, &signed),
        "an assignment verified under a key that did not sign it"
    );
    let bytes = serde_json::to_vec(&signed).expect("an assignment encodes");
    let back: Assignment = serde_json::from_slice(&bytes).expect("an encoded assignment decodes");
    assert!(
        back == signed,
        "an assignment changed on its way through JSON"
    );
    assert!(verify_assignment(public, &back));
}

fn report(r: &Report, key: &SigningKey, public: &[u8; 32], other: &[u8; 32]) {
    let Ok(signature) = sign_report(key, &DIGEST, r) else {
        assert!(!verify_report(public, &DIGEST, r, &"00".repeat(64)));
        return;
    };
    assert!(
        verify_report(public, &DIGEST, r, &signature),
        "a signed report does not verify"
    );
    assert!(
        !verify_report(other, &DIGEST, r, &signature),
        "a report verified under a key that did not sign it"
    );
    assert!(
        !verify_report(public, &[0; 32], r, &signature),
        "a report verified for another assignment"
    );
    let bytes = serde_json::to_vec(r).expect("a report encodes");
    let back: Report = serde_json::from_slice(&bytes).expect("an encoded report decodes");
    assert_eq!(&back, r, "a report changed on its way through JSON");
    assert!(verify_report(public, &DIGEST, &back, &signature));
}
