//! On-disk bytes per state entry under candidate RocksDB column-family options.
//!
//! The in-memory `storage-cost` probe counts rows and value bytes; it cannot
//! see what RocksDB adds (keys, restart points, block trailers, index and
//! filter blocks) or takes away (prefix delta encoding, compression). This
//! tool generates real state rows with `calimero-storage`, writes them into a
//! real RocksDB under the node's `State` key layout (`context_id ‖ tag ‖ id`,
//! 65 bytes), compacts, and reads back the SST bytes.
//!
//! ```text
//! cargo run -p state-disk-cost --release
//! cargo run -p state-disk-cost --release -- --contexts 4 --kv 25000 --chat 10000
//! ```
//!
//! Every variant starts from `calimero_store_rocksdb::{table_options,
//! column_options}`, the options a node opens `State` with, so the `node`
//! row is the production configuration byte for byte.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{AuthoredVector, LwwRegister, Root, UnorderedMap};
use calimero_storage::env::{with_runtime_env, with_seeded_random_bytes, RuntimeEnv};
use calimero_storage::store::{Key, MainStorage};
use eyre::{bail, Result as EyreResult, WrapErr};
use rocksdb::{
    BlockBasedOptions, BottommostLevelCompaction, Cache, ColumnFamilyDescriptor, CompactOptions,
    DBCompressionType, Options, ReadOptions, WriteBatch, DB,
};

/// The column family the node keeps context state in (`Column::State`).
const STATE_CF: &str = "State";

/// Rows per write batch: roughly what one contract call commits.
const ROWS_PER_BATCH: usize = 32;

/// Random point reads timed per variant, against a cold small cache.
const POINT_READS: usize = 20_000;

/// Block cache for the timed reads: far smaller than the data, so reads pay
/// for block decompression the way a node with more state than cache does.
const READ_CACHE_BYTES: usize = 4 * 1024 * 1024;

type Rows = BTreeMap<[u8; 33], Vec<u8>>;

/// One chat message, shaped like mero-chat's: an author-owned entry in an
/// `AuthoredVector` holding a sender, text and a timestamp.
#[derive(BorshSerialize, BorshDeserialize)]
struct Message {
    sender: String,
    text: String,
    timestamp: u64,
    edited: bool,
    reply_to: Option<String>,
}

/// Small deterministic PRNG for message text and write order.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const WORDS: [&str; 48] = [
    "the",
    "a",
    "to",
    "and",
    "is",
    "it",
    "that",
    "for",
    "on",
    "you",
    "we",
    "this",
    "with",
    "can",
    "meeting",
    "tomorrow",
    "sync",
    "node",
    "context",
    "deploy",
    "ok",
    "thanks",
    "sure",
    "later",
    "review",
    "merge",
    "branch",
    "fixed",
    "broken",
    "test",
    "again",
    "looks",
    "good",
    "why",
    "when",
    "build",
    "storage",
    "rocksdb",
    "size",
    "today",
    "yesterday",
    "lunch",
    "call",
    "send",
    "link",
    "please",
    "check",
    "done",
];

const SENDERS: [&str; 5] = [
    "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
    "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
    "4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T",
    "HN7cABqLq46Es1jh92dQQisAq662SmxELLLsHHe4YWrH",
    "2wmVCSfPxGPjrnMMn7rchp4uaeoTqN39mXFC2zhPdri9",
];

fn sentence(rng: &mut XorShift) -> String {
    let words = 3 + rng.below(18);
    (0..words)
        .map(|_| WORDS[rng.below(WORDS.len())])
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run `f` against a fresh in-memory store and return the rows it leaves.
fn rows_of(seed: u64, context_id: [u8; 32], f: impl FnOnce()) -> Rows {
    let rows = Rc::new(RefCell::new(Rows::new()));

    let read = {
        let rows = Rc::clone(&rows);
        Rc::new(move |key: &Key| rows.borrow().get(&key.to_bytes()).cloned())
    };
    let write = {
        let rows = Rc::clone(&rows);
        Rc::new(move |key: Key, value: &[u8]| {
            let _ignored = rows.borrow_mut().insert(key.to_bytes(), value.to_vec());
            true
        })
    };
    let remove = {
        let rows = Rc::clone(&rows);
        Rc::new(move |key: &Key| rows.borrow_mut().remove(&key.to_bytes()).is_some())
    };

    let env = RuntimeEnv::new(read, write, remove, context_id, [2; 32], [3; 32]);
    with_runtime_env(env, || with_seeded_random_bytes(seed, f));

    Rc::try_unwrap(rows)
        .map(RefCell::into_inner)
        .unwrap_or_else(|rows| rows.borrow().clone())
}

/// `n` kv-store entries: `UnorderedMap<String, LwwRegister<String>>`, the
/// layout of `apps/kv-store`.
fn kv_rows(seed: u64, context_id: [u8; 32], n: usize) -> Rows {
    rows_of(seed, context_id, || {
        let mut map = Root::new(UnorderedMap::<String, LwwRegister<String>, MainStorage>::new);
        for i in 0..n {
            let _previous = map
                .insert(format!("key-{i}"), LwwRegister::new(format!("value-{i}")))
                .expect("insert");
        }
        map.commit();
    })
}

/// `n` chat messages pushed onto an `AuthoredVector<Message>`.
fn chat_rows(seed: u64, context_id: [u8; 32], n: usize) -> Rows {
    let mut rng = XorShift(seed | 1);
    rows_of(seed, context_id, || {
        let mut messages = Root::new(AuthoredVector::<Message, MainStorage>::new);
        for i in 0..n {
            let reply_to = (rng.below(8) == 0).then(|| format!("msg-{}", rng.below(i + 1)));
            let _id = messages
                .push(Message {
                    sender: SENDERS[rng.below(SENDERS.len())].to_owned(),
                    text: sentence(&mut rng),
                    timestamp: 1_790_000_000_000 + i as u64 * 1_500 + rng.below(1_000) as u64,
                    edited: rng.below(20) == 0,
                    reply_to,
                })
                .expect("push");
        }
        messages.commit();
    })
}

/// A workload: rows per context, ready to be written under node keys.
struct Workload {
    name: &'static str,
    /// Entries generated (kv entries or messages), across all contexts.
    entries: usize,
    /// `(node key, value)` in a shuffled write order.
    rows: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Workload {
    fn build(name: &'static str, contexts: usize, per_context: usize) -> Self {
        let mut rows = Vec::new();
        for c in 0..contexts {
            let mut context_id = [0_u8; 32];
            let mut rng = XorShift(0x9e37_79b9_7f4a_7c15 ^ (c as u64 + 1));
            for chunk in context_id.chunks_mut(8) {
                chunk.copy_from_slice(&rng.next().to_le_bytes());
            }
            let seed = 0xc057 + c as u64;
            let state = match name {
                "kv" => kv_rows(seed, context_id, per_context),
                _ => chat_rows(seed, context_id, per_context),
            };
            rows.extend(state.into_iter().map(|(key, value)| {
                let mut node_key = context_id.to_vec();
                node_key.extend_from_slice(&key);
                (node_key, value)
            }));
        }
        // Ids are random, so a node writes them in no particular key order.
        let mut rng = XorShift(0x5eed);
        for i in (1..rows.len()).rev() {
            rows.swap(i, rng.below(i + 1));
        }
        Self {
            name,
            entries: contexts * per_context,
            rows,
        }
    }

    fn key_bytes(&self) -> usize {
        self.rows.iter().map(|(k, _)| k.len()).sum()
    }

    fn value_bytes(&self) -> usize {
        self.rows.iter().map(|(_, v)| v.len()).sum()
    }
}

/// A candidate option set: tweaks applied on top of the node's own options.
struct Variant {
    name: &'static str,
    table: fn(&mut BlockBasedOptions),
    column: fn(&mut Options),
}

fn none_table(_: &mut BlockBasedOptions) {}
fn none_column(_: &mut Options) {}

fn variants() -> Vec<Variant> {
    vec![
        Variant {
            name: "uncompressed",
            table: none_table,
            column: |o| {
                o.set_compression_type(DBCompressionType::None);
                o.set_bottommost_compression_type(DBCompressionType::None);
            },
        },
        Variant {
            name: "node",
            table: none_table,
            column: none_column,
        },
        Variant {
            name: "no-dict",
            table: none_table,
            column: |o| o.set_bottommost_compression_options(-14, 3, 0, 0, true),
        },
        Variant {
            name: "zstd9",
            table: none_table,
            column: |o| o.set_bottommost_compression_options(-14, 9, 0, 16 * 1024, true),
        },
        Variant {
            name: "restart32",
            table: |t| t.set_block_restart_interval(32),
            column: none_column,
        },
        Variant {
            name: "restart64",
            table: |t| t.set_block_restart_interval(64),
            column: none_column,
        },
        Variant {
            name: "block16k",
            table: |t| t.set_block_size(16 * 1024),
            column: none_column,
        },
        Variant {
            name: "block64k",
            table: |t| t.set_block_size(64 * 1024),
            column: none_column,
        },
        Variant {
            name: "ribbon",
            table: |t| t.set_ribbon_filter(10.0),
            column: none_column,
        },
        Variant {
            name: "no-bottom-filter",
            table: none_table,
            column: |o| o.set_optimize_filters_for_hits(true),
        },
        Variant {
            name: "block16k+r32+ribbon",
            table: |t| {
                t.set_block_size(16 * 1024);
                t.set_block_restart_interval(32);
                t.set_ribbon_filter(10.0);
            },
            column: none_column,
        },
    ]
}

struct Measured {
    /// SST bytes after flush, before compaction (L0, LZ4).
    flushed: u64,
    /// SST bytes after a forced full compaction (bottommost, ZSTD).
    compacted: u64,
    live_estimate: u64,
    data_blocks: u64,
    index_blocks: u64,
    filter_blocks: u64,
    load: Duration,
    point_read: Duration,
    scan: Duration,
}

fn open(path: &Path, cache: &Cache, variant: &Variant) -> EyreResult<DB> {
    let mut table = calimero_store_rocksdb::table_options(cache);
    (variant.table)(&mut table);
    let mut cf = calimero_store_rocksdb::column_options(&table);
    (variant.column)(&mut cf);

    let mut db_options = Options::default();
    db_options.create_if_missing(true);
    db_options.create_missing_column_families(true);
    Ok(DB::open_cf_descriptors(
        &db_options,
        path,
        [ColumnFamilyDescriptor::new(STATE_CF, cf)],
    )?)
}

fn int_property(db: &DB, name: &str) -> EyreResult<u64> {
    let Some(cf) = db.cf_handle(STATE_CF) else {
        bail!("no {STATE_CF} column family");
    };
    db.property_int_value_cf(cf, name)?
        .ok_or_else(|| eyre::eyre!("property {name} not reported"))
}

/// `name: value` from `rocksdb.aggregated-table-properties`.
fn table_property(db: &DB, name: &str) -> EyreResult<u64> {
    let Some(cf) = db.cf_handle(STATE_CF) else {
        bail!("no {STATE_CF} column family");
    };
    let text = db
        .property_value_cf(cf, "rocksdb.aggregated-table-properties")?
        .unwrap_or_default();
    for field in text.split(';') {
        if let Some((key, value)) = field.split_once('=') {
            if key.trim() == name {
                return value.trim().parse().wrap_err(name.to_owned());
            }
        }
    }
    bail!("table property {name} not in {text:?}")
}

fn measure(workload: &Workload, variant: &Variant) -> EyreResult<Measured> {
    let dir = tempfile::tempdir()?;
    let cache = Cache::new_lru_cache(128 * 1024 * 1024);

    let start = Instant::now();
    {
        let db = open(dir.path(), &cache, variant)?;
        let Some(cf) = db.cf_handle(STATE_CF) else {
            bail!("no {STATE_CF} column family");
        };
        for chunk in workload.rows.chunks(ROWS_PER_BATCH) {
            let mut batch = WriteBatch::default();
            for (key, value) in chunk {
                batch.put_cf(cf, key, value);
            }
            db.write(batch)?;
        }
        db.flush_cf(cf)?;
    }
    let load = start.elapsed();

    let (flushed, compacted, live_estimate, data_blocks, index_blocks, filter_blocks);
    {
        let db = open(dir.path(), &cache, variant)?;
        flushed = int_property(&db, "rocksdb.total-sst-files-size")?;
        let Some(cf) = db.cf_handle(STATE_CF) else {
            bail!("no {STATE_CF} column family");
        };
        let mut compact = CompactOptions::default();
        compact.set_bottommost_level_compaction(BottommostLevelCompaction::Force);
        db.compact_range_cf_opt(cf, None::<&[u8]>, None::<&[u8]>, &compact);
        compacted = int_property(&db, "rocksdb.total-sst-files-size")?;
        live_estimate = int_property(&db, "rocksdb.estimate-live-data-size")?;
        data_blocks = table_property(&db, "data block size")?;
        index_blocks = table_property(&db, "index block size (user-key? 0, delta-value? 0)")
            .or_else(|_| table_property(&db, "index block size"))?;
        filter_blocks = table_property(&db, "filter block size")?;
    }

    // Reads against a reopened DB with a cache much smaller than the data.
    let small = Cache::new_lru_cache(READ_CACHE_BYTES);
    let db = open(dir.path(), &small, variant)?;
    let Some(cf) = db.cf_handle(STATE_CF) else {
        bail!("no {STATE_CF} column family");
    };
    let mut rng = XorShift(0xbeef);
    let start = Instant::now();
    for _ in 0..POINT_READS {
        let (key, _) = &workload.rows[rng.below(workload.rows.len())];
        if db.get_pinned_cf(cf, key)?.is_none() {
            bail!("written key missing");
        }
    }
    let point_read = start.elapsed();

    let start = Instant::now();
    let mut iter = db.raw_iterator_cf_opt(cf, ReadOptions::default());
    iter.seek_to_first();
    let mut seen = 0_usize;
    while iter.valid() {
        seen += 1;
        iter.next();
    }
    iter.status()?;
    let scan = start.elapsed();
    if seen != workload.rows.len() {
        bail!("scan saw {seen} rows, wrote {}", workload.rows.len());
    }

    Ok(Measured {
        flushed,
        compacted,
        live_estimate,
        data_blocks,
        index_blocks,
        filter_blocks,
        load,
        point_read,
        scan,
    })
}

fn arg(args: &[String], flag: &str, default: usize) -> EyreResult<usize> {
    match args.iter().position(|a| a == flag) {
        Some(i) => args
            .get(i + 1)
            .ok_or_else(|| eyre::eyre!("{flag} needs a value"))?
            .parse()
            .wrap_err(flag.to_owned()),
        None => Ok(default),
    }
}

fn main() -> EyreResult<()> {
    let args: Vec<String> = std::env::args().collect();
    let contexts = arg(&args, "--contexts", 4)?;
    let kv = arg(&args, "--kv", 25_000)?;
    let chat = arg(&args, "--chat", 10_000)?;
    let only = args
        .iter()
        .position(|a| a == "--only")
        .and_then(|i| args.get(i + 1));

    let workloads = [
        Workload::build("kv", contexts, kv),
        Workload::build("chat", contexts, chat),
    ];

    for workload in &workloads {
        let entries = workload.entries as f64;
        println!(
            "\n## {} — {} contexts, {} entries, {} rows, {:.1} rows/entry, logical {:.0} B/entry \
             (keys {:.0} + values {:.0})\n",
            workload.name,
            contexts,
            workload.entries,
            workload.rows.len(),
            workload.rows.len() as f64 / entries,
            (workload.key_bytes() + workload.value_bytes()) as f64 / entries,
            workload.key_bytes() as f64 / entries,
            workload.value_bytes() as f64 / entries,
        );
        println!(
            "| variant | flushed B/entry | compacted B/entry | vs node | live est. B/entry | \
             data / index / filter B/entry | load ms | {POINT_READS} gets ms | scan ms |"
        );
        println!("|---|---:|---:|---:|---:|---|---:|---:|---:|");

        let mut node = None;
        for variant in variants() {
            if only.is_some_and(|o| !variant.name.contains(o.as_str())) {
                continue;
            }
            let m = measure(workload, &variant)?;
            if variant.name == "node" {
                node = Some(m.compacted);
            }
            let vs = node.map_or_else(
                || "—".to_owned(),
                |n| format!("{:+.1}%", (m.compacted as f64 / n as f64 - 1.0) * 100.0),
            );
            println!(
                "| {} | {:.1} | {:.1} | {} | {:.1} | {:.1} / {:.1} / {:.1} | {} | {} | {} |",
                variant.name,
                m.flushed as f64 / entries,
                m.compacted as f64 / entries,
                vs,
                m.live_estimate as f64 / entries,
                m.data_blocks as f64 / entries,
                m.index_blocks as f64 / entries,
                m.filter_blocks as f64 / entries,
                m.load.as_millis(),
                m.point_read.as_millis(),
                m.scan.as_millis(),
            );
        }
    }

    Ok(())
}
