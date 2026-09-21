//! The bytes one keystroke rewrites must not grow with the run it lands in.
//!
//! No row counter can see this. Appending to a `FugueText` run rewrites exactly
//! one block row however long that run has grown, so `fugue_text_insert_per_char`
//! reported a flat cost while the bytes written per keystroke grew linearly with
//! the document. This is the gate on `MAX_RUN_LEN`; the snapshot cannot be, since
//! byte counts are not reproducible enough to pin exactly (see `lib.rs`).

use calimero_storage::collections::{FugueText, Root};
use calimero_storage::store::MainStorage;
use storage_cost::{measure, reset_counters};

const SHORT_RUN: usize = 1_000; // characters typed before the measured keystroke
const LONG_RUN: usize = 10_000; // ten times the run, one identical keystroke
const MAX_GROWTH: f64 = 1.5; // random-id drift headroom; an uncapped run costs 5x

/// Bytes written by ONE append after `run` characters have been typed.
fn keystroke_bytes(run: usize) -> u64 {
    let (_, costs) = measure(|| {
        let mut doc = Root::new(|| FugueText::<MainStorage>::new_with_field_name("keystroke"));
        for i in 0..run {
            doc.insert(i, 'a').expect("insert should succeed");
        }
        reset_counters();
        doc.insert(run, 'z').expect("insert should succeed");
    });
    costs.bytes_written
}

/// `#[ignore]`d for `tests/reproducible.rs`'s reason: typing `LONG_RUN`
/// characters costs 200s in the debug profile the workspace-wide `cargo test`
/// uses. The release storage-cost job runs it via `--include-ignored`.
#[ignore = "slow: types 11,000 characters; run by the release storage-cost CI job via --include-ignored"]
#[test]
fn bytes_written_per_keystroke_do_not_grow_with_the_run() {
    let short = keystroke_bytes(SHORT_RUN);
    let long = keystroke_bytes(LONG_RUN);
    assert!(
        long as f64 <= short as f64 * MAX_GROWTH,
        "one keystroke wrote {short} bytes after {SHORT_RUN} characters and {long} after \
         {LONG_RUN} - a run a keystroke rewrites whole is unbounded again"
    );
}
