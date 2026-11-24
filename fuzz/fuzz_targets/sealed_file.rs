#![no_main]
//! Sealed files: a node opens the chunked AES-GCM stream of every file it is
//! handed back (a job node's inputs, an upload's parts, a run's outputs) with
//! `FileOpener`, fed in whatever pieces the network delivers, from bytes
//! whoever stored them could change.
//!
//! The first input byte picks the mode, the second and third the size of the
//! pieces the stream is fed in. Either the rest is a stream as the fuzzer
//! wrote it, which never opens to a whole file, or it is a plaintext (repeated
//! by the fourth byte, so streams cross chunk boundaries) that the target
//! seals and then alters as the fifth to eighth bytes say. A stream opens to
//! its plaintext exactly when nothing was altered, and the pieces it is fed in
//! change nothing.

use calimero_sealed::file::{FileOpener, CHUNK_BYTES};
use calimero_sealed::fuzz_api::sealer;
use libfuzzer_sys::fuzz_target;
use sha2::{Digest, Sha256};

const KEY: [u8; 32] = [0x6b; 32];

fuzz_target!(|data: &[u8]| {
    let Some((&[mode, step_hi, step_lo], rest)) = data.split_first_chunk::<3>() else {
        return;
    };
    let step = usize::from(u16::from_be_bytes([step_hi, step_lo])).max(1);
    if mode & 1 == 0 {
        // As the fuzzer wrote it: it cannot forge a tag, and pieces change nothing.
        let whole = open(rest, rest.len().max(1), 0, &[0; 32]);
        let pieces = open(rest, step, 0, &[0; 32]);
        assert_eq!(
            whole, pieces,
            "the pieces a stream came in changed what opened"
        );
        assert!(whole.1.is_none(), "a stream the fuzzer wrote opened");
        return;
    }
    let Some((&[repeat, op, at_hi, at_lo], plain)) = rest.split_first_chunk::<4>() else {
        return;
    };
    let plain = plain.repeat(usize::from(repeat % 8).max(1) * if repeat >= 128 { 64 } else { 1 });
    let mut s = sealer(KEY);
    let mut stream = s.write(&plain).expect("seals");
    let sealed = s.finish().expect("seals");
    stream.extend_from_slice(&sealed.tail);
    assert_eq!(sealed.size, plain.len() as u64);

    let original = stream.clone();
    let at = usize::from(u16::from_be_bytes([at_hi, at_lo])) * 61 % stream.len().max(1);
    match op % 4 {
        0 => {}
        1 => stream[at] ^= 1 << ((op >> 2) & 7),
        2 => stream.truncate(at),
        _ => stream.insert(at, op),
    }
    let altered = stream != original;
    let (prefix, opened) = open(&stream, step, sealed.size, &sealed.sha256);
    assert!(
        plain.starts_with(&prefix),
        "a stream opened to bytes that were never sealed"
    );
    if altered {
        assert!(opened.is_none(), "an altered stream opened");
    } else {
        assert_eq!(
            opened.as_deref(),
            Some(plain.as_slice()),
            "a sealed stream did not open"
        );
        let chunks = plain.len().div_ceil(CHUNK_BYTES).max(1);
        assert_eq!(original.len(), plain.len() + 16 * chunks);
    }
});

/// What `FileOpener` gives, fed `stream` in pieces of `step` bytes: the plaintext it
/// released before the end, and the whole file if it finished as `size` and `sha256`.
fn open(stream: &[u8], step: usize, size: u64, sha256: &[u8; 32]) -> (Vec<u8>, Option<Vec<u8>>) {
    let mut opener = FileOpener::new(&KEY);
    let mut out = Vec::new();
    for piece in stream.chunks(step) {
        match opener.push(piece) {
            Ok(plain) => out.extend_from_slice(&plain),
            Err(_) => return (out, None),
        }
    }
    let Ok(last) = opener.finish_expecting(size, sha256) else {
        return (out, None);
    };
    let mut whole = out.clone();
    whole.extend_from_slice(&last);
    assert_eq!(Sha256::digest(&whole).as_slice(), sha256.as_slice());
    (out, Some(whole))
}
