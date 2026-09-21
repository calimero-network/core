//! The gate on `MAX_RUN_LEN`: the bytes one keystroke rewrites must not grow
//! with the run it lands in. Row counters cannot see it - a run is one row.

use calimero_storage::collections::{FugueText, Root};
use calimero_storage::store::MainStorage;
use storage_cost::{measure, reset_counters};

const SHORT_RUN: usize = 1_000; // characters typed before the measured keystroke
const LONG_RUN: usize = 10_000; // ten times the run, one identical keystroke
const MAX_GROWTH: f64 = 1.5; // random-id drift headroom; an uncapped run costs 5x

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
