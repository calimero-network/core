//! Emit the storage-cost table as JSON on stdout.
//!
//! `scripts/check-storage-cost.sh` diffs this against the committed snapshot.
//! Output is deterministically ordered (`BTreeMap`, zero-padded numeric keys)
//! so a diff shows real cost changes and never key reordering.
//!
//! Only ROW counters are emitted. Byte counters are not reproducible — entity
//! ids are random, so index rows serialize to slightly different lengths run to
//! run. See the module docs of `storage_cost` for the full reasoning; gating on
//! bytes would be a flake generator.
//!
//! `--workload <name>` restricts the run to one workload.
//!
//! Each workload also carries the tolerance the gate applies to it (see
//! `workloads::Workload::tolerance_pct`); it is emitted here so regenerating
//! the snapshot cannot drop it.

use std::collections::BTreeMap;

use serde::Serialize;
use storage_cost::workloads::all;
use storage_cost::{measure, RowCosts};

/// One workload's snapshot entry: the tolerance the gate applies, then the
/// measured costs per collection size.
///
/// The tolerance is emitted rather than hand-maintained in the JSON so that
/// regenerating the snapshot cannot silently drop it, and so that changing it
/// is a code change a reviewer sees in `workloads.rs`.
#[derive(Serialize)]
struct Entry {
    tolerance_pct: u32,
    sizes: BTreeMap<String, RowCosts>,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut only: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workload" => {
                only = Some(args.next().unwrap_or_else(|| {
                    eprintln!("--workload requires a value");
                    std::process::exit(2);
                }));
            }
            other => {
                eprintln!("unknown argument: {other}");
                eprintln!("usage: storage-cost [--workload <name>]");
                std::process::exit(2);
            }
        }
    }

    let mut table: BTreeMap<String, Entry> = BTreeMap::new();
    for workload in all() {
        if only.as_ref().is_some_and(|name| workload.name != name) {
            continue;
        }
        let (_, costs) = measure(|| (workload.run)(workload.n));
        let entry = table.entry(workload.name.to_owned()).or_insert(Entry {
            tolerance_pct: workload.tolerance_pct,
            sizes: BTreeMap::new(),
        });
        // Zero-padded so string ordering matches numeric ordering in the
        // committed snapshot — "10" must not sort before "1000".
        let _ignored = entry
            .sizes
            .insert(format!("{:08}", workload.n), costs.rows());
    }

    if table.is_empty() {
        eprintln!(
            "no workloads matched{}",
            only.map(|n| format!(" --workload {n}")).unwrap_or_default()
        );
        std::process::exit(2);
    }

    let json = serde_json::to_string_pretty(&table).expect("RowCosts serialization is infallible");
    println!("{json}");
}
