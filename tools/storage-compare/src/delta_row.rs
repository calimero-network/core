//! What one call's `Column::Delta` row costs, field by field, in the layout rows
//! were written in before (borsh of the old `ContextDagDelta`, which led with the
//! delta id) and in the current one (`ContextDagDelta::to_row_bytes`).
//!
//! The rows are built the way `execute` persists a self-authored delta: the
//! storage layer's real actions for the call, a single parent (the previous
//! call's delta), the call's HLC, `applied`, no events, the author's device key,
//! the governance position (`borsh(GovernanceParentEdge)` with one head, the
//! steady state) and a 64-byte signature. Ids, keys and signatures are random
//! bytes, as hashes and Ed25519 signatures are to a compressor.
use std::time::Instant;

use borsh::BorshSerialize;
use calimero_context_config::types::GovernanceParentEdge;
use calimero_primitives::identity::PublicKey;
use calimero_storage::collections::{LwwRegister, Root};
use calimero_storage::env::{hlc_timestamp, take_last_artifact};
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::types::ContextDagDelta;
use rocksdb::{
    BottommostLevelCompaction, ColumnFamilyDescriptor, CompactOptions, DBCompressionType, Options,
    WriteBatch, DB,
};

use crate::{config, message, rows_of, Chat, Kv, XorShift, PREFILL};

/// Deltas a context retains (DAG compaction keeps up to about this many).
pub const ROWS: usize = 10_000;

/// Authors writing into the measured context.
const AUTHORS: usize = 3;

/// The old row layout, field for field: what `Column::Delta` held before the
/// current layout. Only this probe writes it, to measure it.
#[derive(BorshSerialize)]
struct LegacyRow {
    delta_id: [u8; 32],
    parents: Vec<[u8; 32]>,
    actions: Vec<u8>,
    hlc: HybridTimestamp,
    applied: bool,
    checkpoint_root_hash: Option<[u8; 32]>,
    events: Option<Vec<u8>>,
    author_id: Option<PublicKey>,
    governance_position_blob: Option<Vec<u8>>,
    delta_signature: Option<[u8; 64]>,
    delegation: Option<calimero_account::Delegation>,
}

impl LegacyRow {
    fn of(delta_id: [u8; 32], row: &ContextDagDelta) -> Self {
        Self {
            delta_id,
            parents: row.parents.clone(),
            actions: row.actions.clone(),
            hlc: row.hlc,
            applied: row.applied,
            checkpoint_root_hash: row.checkpoint_root_hash,
            events: row.events.clone(),
            author_id: row.author_id,
            governance_position_blob: row.governance_position_blob.clone(),
            delta_signature: row.delta_signature,
            delegation: row.delegation.clone(),
        }
    }
}

#[derive(Clone, Copy)]
pub enum Workload {
    KvSet,
    KvUpdate,
    Chat,
}

impl Workload {
    pub const ALL: [Self; 3] = [Self::KvSet, Self::KvUpdate, Self::Chat];

    pub const fn name(self) -> &'static str {
        match self {
            Self::KvSet => "kv set (new key)",
            Self::KvUpdate => "kv update (existing key)",
            Self::Chat => "chat send",
        }
    }
}

/// `ROWS` calls of `workload` after the usual prefill: each call's actions
/// (`borsh(Vec<Action>)`, the artifact without its `StorageDelta` tag) and HLC.
fn calls(workload: Workload) -> Vec<(Vec<u8>, HybridTimestamp)> {
    let mut rng = XorShift(0xde17a);
    rows_of(0xde17a, || {
        match workload {
            Workload::KvSet | Workload::KvUpdate => {
                let mut map = Root::new(Kv::new);
                for i in 0..ROWS.max(PREFILL) {
                    let _ = map
                        .insert(format!("key-{i}"), LwwRegister::new(format!("value-{i}")))
                        .expect("insert");
                }
                map.commit();
            }
            Workload::Chat => {
                let mut chat = Root::new(Chat::new);
                for i in 0..PREFILL {
                    let _ = chat.push(message(&mut rng, i)).expect("push");
                }
                chat.commit();
            }
        }
        (0..ROWS)
            .map(|i| {
                let _ = take_last_artifact();
                match workload {
                    Workload::KvSet => {
                        let mut map = Root::<Kv>::fetch().expect("root");
                        let _ = map
                            .insert(
                                format!("new-key-{i}"),
                                LwwRegister::new(format!("value-{i}")),
                            )
                            .expect("insert");
                        map.commit();
                    }
                    Workload::KvUpdate => {
                        let mut map = Root::<Kv>::fetch().expect("root");
                        map.get_mut(&format!("key-{i}"))
                            .expect("get_mut")
                            .expect("present")
                            .set(format!("new-value-{i}"));
                        map.commit();
                    }
                    Workload::Chat => {
                        let m = message(&mut rng, PREFILL + i);
                        let mut chat = Root::<Chat>::fetch().expect("root");
                        let _ = chat.push(m).expect("push");
                        chat.commit();
                    }
                }
                let artifact = take_last_artifact().expect("delta");
                // `StorageDelta::Actions` is variant 0; the row stores its vector.
                assert_eq!(artifact[0], 0, "a plain `StorageDelta::Actions`");
                (artifact[1..].to_vec(), hlc_timestamp())
            })
            .collect()
    })
    .0
}

fn random<const N: usize>(rng: &mut XorShift) -> [u8; N] {
    let mut out = [0; N];
    for chunk in out.chunks_mut(8) {
        let word = rng.next().to_le_bytes();
        chunk.copy_from_slice(&word[..chunk.len()]);
    }
    out
}

/// `(delta_id, row)` for each call, chained parent to child.
pub fn rows(workload: Workload) -> Vec<([u8; 32], ContextDagDelta)> {
    let mut rng = XorShift(0x0005_eed0 + workload as u64);
    let authors: Vec<PublicKey> = (0..AUTHORS)
        .map(|_| PublicKey::from(random::<32>(&mut rng)))
        .collect();
    let governance = borsh::to_vec(&GovernanceParentEdge {
        governance_dag_heads: vec![random(&mut rng)],
    })
    .expect("governance position");
    let mut parent = random::<32>(&mut rng);
    calls(workload)
        .into_iter()
        .enumerate()
        .map(|(i, (actions, hlc))| {
            let id = random::<32>(&mut rng);
            let row = ContextDagDelta {
                parents: vec![parent],
                actions,
                hlc,
                applied: true,
                checkpoint_root_hash: None,
                events: None,
                author_id: Some(authors[i % AUTHORS]),
                governance_position_blob: Some(governance.clone()),
                delta_signature: Some(random(&mut rng)),
                delegation: None,
            };
            parent = id;
            (id, row)
        })
        .collect()
}

/// Bytes of an unsigned LEB128 length.
const fn varint_len(mut n: usize) -> usize {
    let mut len = 1;
    while n >= 0x80 {
        n >>= 7;
        len += 1;
    }
    len
}

/// The fields a row's bytes are split into, in report order.
pub const FIELDS: [&str; 12] = [
    "header (v1: tag, version, flags)",
    "delta_id",
    "parents",
    "actions",
    "hlc",
    "applied",
    "checkpoint_root_hash",
    "events",
    "author_id",
    "governance_position_blob",
    "delta_signature",
    "delegation",
];

/// Per-field bytes of `row` in the old layout, checked against its encoding.
pub fn legacy_fields(id: [u8; 32], row: &ContextDagDelta) -> [usize; 12] {
    let opt = |present: bool, len: usize| 1 + if present { len } else { 0 };
    let fields = [
        0,
        32,
        4 + 32 * row.parents.len(),
        4 + row.actions.len(),
        16,
        1,
        opt(row.checkpoint_root_hash.is_some(), 32),
        opt(
            row.events.is_some(),
            4 + row.events.as_ref().map_or(0, Vec::len),
        ),
        opt(row.author_id.is_some(), 32),
        opt(
            row.governance_position_blob.is_some(),
            4 + row.governance_position_blob.as_ref().map_or(0, Vec::len),
        ),
        opt(row.delta_signature.is_some(), 64),
        opt(
            row.delegation.is_some(),
            row.delegation
                .as_ref()
                .map_or(0, |d| borsh::to_vec(d).expect("delegation").len()),
        ),
    ];
    let encoded = borsh::to_vec(&LegacyRow::of(id, row)).expect("legacy row");
    assert_eq!(fields.iter().sum::<usize>(), encoded.len(), "legacy split");
    fields
}

/// Per-field bytes of `row` in the current layout, checked against its encoding.
pub fn v1_fields(row: &ContextDagDelta) -> [usize; 12] {
    let opt = |present: bool, len: usize| if present { len } else { 0 };
    let bytes = |b: &Option<Vec<u8>>| b.as_ref().map_or(0, |b| varint_len(b.len()) + b.len());
    let fields = [
        3,
        0,
        varint_len(row.parents.len()) + 32 * row.parents.len(),
        varint_len(row.actions.len()) + row.actions.len(),
        16,
        0,
        opt(row.checkpoint_root_hash.is_some(), 32),
        bytes(&row.events),
        opt(row.author_id.is_some(), 32),
        bytes(&row.governance_position_blob),
        opt(row.delta_signature.is_some(), 64),
        row.delegation
            .as_ref()
            .map_or(0, |d| borsh::to_vec(d).expect("delegation").len()),
    ];
    let encoded = row.to_row_bytes().expect("row");
    assert_eq!(fields.iter().sum::<usize>(), encoded.len(), "v1 split");
    fields
}

pub struct Disk {
    pub bytes: u64,
    pub compact_ms: f64,
}

/// Write `rows` under one context's `context_id ‖ delta_id` keys into a fresh
/// RocksDB `Delta` family, one row per write batch as the node commits them,
/// compact fully, and return the SST bytes.
pub fn on_disk(rows: &[([u8; 32], Vec<u8>)], cf: Options) -> Disk {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut db_opts = Options::default();
    db_opts.create_if_missing(true);
    db_opts.create_missing_column_families(true);
    let db = DB::open_cf_descriptors(
        &db_opts,
        dir.path(),
        [ColumnFamilyDescriptor::new("Delta", cf)],
    )
    .expect("open");
    let h = db.cf_handle("Delta").expect("cf");
    let context = [0x42_u8; 32];
    for (id, value) in rows {
        let mut key = context.to_vec();
        key.extend_from_slice(id);
        let mut b = WriteBatch::default();
        b.put_cf(h, key, value);
        db.write(b).expect("write");
    }
    db.flush_cf(h).expect("flush");
    let mut c = CompactOptions::default();
    c.set_bottommost_level_compaction(BottommostLevelCompaction::Force);
    let t = Instant::now();
    db.compact_range_cf_opt(h, None::<&[u8]>, None::<&[u8]>, &c);
    let compact_ms = t.elapsed().as_secs_f64() * 1e3;
    Disk {
        bytes: db
            .property_int_value_cf(h, "rocksdb.total-sst-files-size")
            .expect("prop")
            .expect("value"),
        compact_ms,
    }
}

pub fn uncompressed() -> Options {
    let mut raw = Options::default();
    raw.set_compression_type(DBCompressionType::None);
    raw.set_bottommost_compression_type(DBCompressionType::None);
    raw
}

/// The report: a per-field table per workload, then on-disk bytes.
pub fn report() {
    println!("\n## Delta rows ({ROWS} calls per workload, one context)\n");
    let mut disk = Vec::new();
    for workload in Workload::ALL {
        let rows = std::thread::spawn(move || rows(workload))
            .join()
            .expect("rows");
        let n = rows.len() as f64;
        let mut legacy = [0_usize; 12];
        let mut v1 = [0_usize; 12];
        for (id, row) in &rows {
            for (sum, b) in legacy.iter_mut().zip(legacy_fields(*id, row)) {
                *sum += b;
            }
            for (sum, b) in v1.iter_mut().zip(v1_fields(row)) {
                *sum += b;
            }
            // Both layouts read back as the same row.
            let old = borsh::to_vec(&LegacyRow::of(*id, row)).expect("legacy");
            let decoded = ContextDagDelta::from_row_bytes(&old).expect("legacy decodes");
            assert!(&decoded == row, "legacy round trip");
        }
        println!("### {}\n", workload.name());
        println!("| field (mean B/row) | before | after |");
        println!("|---|---:|---:|");
        for (i, field) in FIELDS.iter().enumerate() {
            println!(
                "| {field} | {:.1} | {:.1} |",
                legacy[i] as f64 / n,
                v1[i] as f64 / n
            );
        }
        let (lt, vt) = (legacy.iter().sum::<usize>(), v1.iter().sum::<usize>());
        println!(
            "| **value total** | **{:.1}** | **{:.1}** |",
            lt as f64 / n,
            vt as f64 / n
        );
        println!(
            "| key (context_id ‖ delta_id) | 64 | 64 |\n| **key + value** | **{:.1}** | **{:.1}** |\n",
            lt as f64 / n + 64.0,
            vt as f64 / n + 64.0
        );

        let old: Vec<_> = rows
            .iter()
            .map(|(id, row)| {
                (
                    *id,
                    borsh::to_vec(&LegacyRow::of(*id, row)).expect("legacy"),
                )
            })
            .collect();
        let new: Vec<_> = rows
            .iter()
            .map(|(id, row)| (*id, row.to_row_bytes().expect("row")))
            .collect();
        disk.push((
            workload,
            [
                on_disk(&old, uncompressed()),
                on_disk(&new, uncompressed()),
                on_disk(&old, config::state_options()),
                on_disk(&new, config::state_options()),
            ],
        ));
    }
    println!("### On disk, {ROWS} rows, fully compacted (B/row)\n");
    println!("| workload | uncompressed before | uncompressed after | node options before | node options after | change | compact ms before / after |");
    println!("|---|---:|---:|---:|---:|---:|---:|");
    for (workload, [ub, ua, nb, na]) in disk {
        let per = |d: &Disk| d.bytes as f64 / ROWS as f64;
        println!(
            "| {} | {:.1} | {:.1} | {:.1} | {:.1} | {:+.1}% | {:.0} / {:.0} |",
            workload.name(),
            per(&ub),
            per(&ua),
            per(&nb),
            per(&na),
            (per(&na) / per(&nb) - 1.0) * 100.0,
            nb.compact_ms,
            na.compact_ms
        );
    }
}
