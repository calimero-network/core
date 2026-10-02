//! Same workloads, run against whichever calimero-storage this crate is built in.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Instant;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{AuthoredVector, LwwRegister, Root, UnorderedMap};
use calimero_storage::env::{
    take_last_artifact, with_runtime_env, with_seeded_random_bytes, RuntimeEnv,
};
use calimero_storage::store::{Key, MainStorage};
use rocksdb::{
    BottommostLevelCompaction, ColumnFamilyDescriptor, CompactOptions, DBCompressionType, Options,
    WriteBatch, DB,
};

#[path = "config.rs"]
mod config;
mod delta_row;

type Rows = BTreeMap<Vec<u8>, Vec<u8>>;
type Kv = UnorderedMap<String, LwwRegister<String>, MainStorage>;
type Chat = AuthoredVector<Message, MainStorage>;

#[derive(BorshSerialize, BorshDeserialize)]
struct Message {
    sender: String,
    text: String,
    timestamp: u64,
    edited: bool,
    reply_to: Option<String>,
}

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
const WORDS: [&str; 24] = [
    "the", "a", "to", "and", "is", "meeting", "tomorrow", "sync", "node", "context", "deploy",
    "ok", "thanks", "sure", "review", "merge", "branch", "fixed", "test", "looks", "good",
    "storage", "size", "done",
];
const SENDERS: [&str; 3] = [
    "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
    "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
    "4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T",
];
fn message(rng: &mut XorShift, i: usize) -> Message {
    let words = 3 + rng.below(18);
    Message {
        sender: SENDERS[rng.below(SENDERS.len())].to_owned(),
        text: (0..words)
            .map(|_| WORDS[rng.below(WORDS.len())])
            .collect::<Vec<_>>()
            .join(" "),
        timestamp: 1_790_000_000_000 + i as u64 * 1_500,
        edited: false,
        reply_to: (rng.below(8) == 0).then(|| format!("msg-{}", rng.below(i + 1))),
    }
}

/// Run `f` against a fresh in-memory store; return its rows.
fn rows_of<R>(seed: u64, f: impl FnOnce() -> R) -> (R, Rows) {
    let rows = Rc::new(RefCell::new(Rows::new()));
    let r = Rc::clone(&rows);
    let read = Rc::new(move |k: &Key| r.borrow().get(&k.to_bytes()[..]).cloned());
    let w = Rc::clone(&rows);
    let write = Rc::new(move |k: Key, v: &[u8]| {
        let _ = w.borrow_mut().insert(k.to_bytes().to_vec(), v.to_vec());
        true
    });
    let d = Rc::clone(&rows);
    let remove = Rc::new(move |k: &Key| d.borrow_mut().remove(&k.to_bytes()[..]).is_some());
    let env = RuntimeEnv::new(read, write, remove, [1; 32], [2; 32], [3; 32]);
    let out = with_runtime_env(env, || with_seeded_random_bytes(seed, f));
    let rows = Rc::try_unwrap(rows)
        .map(RefCell::into_inner)
        .unwrap_or_else(|r| r.borrow().clone());
    (out, rows)
}

/// Build `n` entries with one commit per entry (one contract call each),
/// then run `ops` more calls of `op`, returning each call's delta size.
fn kv_state(n: usize) -> Rows {
    rows_of(0xc057, || {
        let mut map = Root::new(Kv::new);
        for i in 0..n {
            let _ = map
                .insert(format!("key-{i}"), LwwRegister::new(format!("value-{i}")))
                .expect("insert");
        }
        map.commit();
    })
    .1
}
fn chat_state(n: usize) -> Rows {
    let mut rng = XorShift(0xc057);
    rows_of(0xc057, || {
        let mut chat = Root::new(Chat::new);
        for i in 0..n {
            let _ = chat.push(message(&mut rng, i)).expect("push");
        }
        chat.commit();
    })
    .1
}

/// Median of per-call delta artifact bytes and per-call wall time (µs).
struct Calls {
    delta_bytes: f64,
    us_per_call: f64,
}
fn summarize(mut bytes: Vec<usize>, mut us: Vec<f64>) -> Calls {
    bytes.sort_unstable();
    us.sort_by(|a, b| a.total_cmp(b));
    Calls {
        delta_bytes: bytes[bytes.len() / 2] as f64,
        us_per_call: us[us.len() / 2],
    }
}
const PREFILL: usize = 1_000;
const CALLS: usize = 500;

fn kv_calls(update: bool) -> Calls {
    rows_of(0xd17a, || {
        let mut map = Root::new(Kv::new);
        for i in 0..PREFILL {
            let _ = map
                .insert(format!("key-{i}"), LwwRegister::new(format!("value-{i}")))
                .expect("insert");
        }
        map.commit();
        let (mut bytes, mut us) = (Vec::new(), Vec::new());
        for i in 0..CALLS {
            let _ = take_last_artifact();
            let t = Instant::now();
            let mut map = Root::<Kv>::fetch().expect("root");
            if update {
                map.get_mut(&format!("key-{i}"))
                    .expect("get_mut")
                    .expect("present")
                    .set(format!("new-value-{i}"));
            } else {
                let _ = map
                    .insert(
                        format!("new-key-{i}"),
                        LwwRegister::new(format!("value-{i}")),
                    )
                    .expect("insert");
            }
            map.commit();
            us.push(t.elapsed().as_secs_f64() * 1e6);
            bytes.push(take_last_artifact().expect("delta").len());
        }
        summarize(bytes, us)
    })
    .0
}
fn chat_calls() -> Calls {
    let mut rng = XorShift(0xd17a);
    rows_of(0xd17a, || {
        let mut chat = Root::new(Chat::new);
        for i in 0..PREFILL {
            let _ = chat.push(message(&mut rng, i)).expect("push");
        }
        chat.commit();
        let (mut bytes, mut us) = (Vec::new(), Vec::new());
        for i in 0..CALLS {
            let _ = take_last_artifact();
            let m = message(&mut rng, PREFILL + i);
            let t = Instant::now();
            let mut chat = Root::<Chat>::fetch().expect("root");
            let _ = chat.push(m).expect("push");
            chat.commit();
            us.push(t.elapsed().as_secs_f64() * 1e6);
            bytes.push(take_last_artifact().expect("delta").len());
        }
        summarize(bytes, us)
    })
    .0
}

/// Write `contexts` copies of `rows` under 32-byte context prefixes into a
/// fresh RocksDB, compact fully, and return SST bytes.
fn on_disk(rows: &Rows, contexts: usize, mut cf: Options) -> u64 {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut all = Vec::new();
    for c in 0..contexts {
        let mut rng = XorShift(0x9e37_79b9 ^ (c as u64 + 1));
        let mut ctx = [0_u8; 32];
        for chunk in ctx.chunks_mut(8) {
            chunk.copy_from_slice(&rng.next().to_le_bytes());
        }
        all.extend(rows.iter().map(|(k, v)| {
            let mut key = ctx.to_vec();
            key.extend_from_slice(k);
            (key, v.clone())
        }));
    }
    let mut rng = XorShift(0x5eed);
    for i in (1..all.len()).rev() {
        all.swap(i, rng.below(i + 1));
    }
    cf.create_if_missing(true);
    let mut db_opts = Options::default();
    db_opts.create_if_missing(true);
    db_opts.create_missing_column_families(true);
    let db = DB::open_cf_descriptors(
        &db_opts,
        dir.path(),
        [ColumnFamilyDescriptor::new("State", cf)],
    )
    .expect("open");
    let h = db.cf_handle("State").expect("cf");
    for chunk in all.chunks(32) {
        let mut b = WriteBatch::default();
        for (k, v) in chunk {
            b.put_cf(h, k, v);
        }
        db.write(b).expect("write");
    }
    db.flush_cf(h).expect("flush");
    let mut c = CompactOptions::default();
    c.set_bottommost_level_compaction(BottommostLevelCompaction::Force);
    db.compact_range_cf_opt(h, None::<&[u8]>, None::<&[u8]>, &c);
    db.property_int_value_cf(h, "rocksdb.total-sst-files-size")
        .expect("prop")
        .expect("value")
}

fn main() {
    if std::env::args().any(|a| a == "--deltas") {
        delta_row::report();
        return;
    }
    const CONTEXTS: usize = 4;
    let kv_n = 25_000;
    let chat_n = 10_000;
    println!("# {}\n", config::LABEL);
    println!("| workload | rows/entry | logical B/entry (key+value) | on disk B/entry, uncompressed | on disk B/entry, production options |");
    println!("|---|---:|---:|---:|---:|");
    for (name, n, rows) in [
        (
            "kv-store entry",
            kv_n,
            std::thread::spawn(move || kv_state(kv_n))
                .join()
                .expect("kv"),
        ),
        (
            "chat message",
            chat_n,
            std::thread::spawn(move || chat_state(chat_n))
                .join()
                .expect("chat"),
        ),
    ] {
        let logical: usize = rows.iter().map(|(k, v)| 32 + k.len() + v.len()).sum();
        let mut raw = Options::default();
        raw.set_compression_type(DBCompressionType::None);
        raw.set_bottommost_compression_type(DBCompressionType::None);
        let total = (n * CONTEXTS) as f64;
        println!(
            "| {name} | {:.2} | {:.1} | {:.1} | {:.1} |",
            rows.len() as f64 / n as f64,
            logical as f64 / n as f64,
            on_disk(&rows, CONTEXTS, raw) as f64 / total,
            on_disk(&rows, CONTEXTS, config::state_options()) as f64 / total
        );
    }
    println!("\n| call ({PREFILL} prefilled, median of {CALLS}) | delta artifact B | µs per call (in-memory store) |");
    println!("|---|---:|---:|");
    for (name, c) in [
        (
            "kv set (new key)",
            std::thread::spawn(|| kv_calls(false)).join().expect("kv"),
        ),
        (
            "kv update (existing key)",
            std::thread::spawn(|| kv_calls(true)).join().expect("kv"),
        ),
        (
            "chat send",
            std::thread::spawn(chat_calls).join().expect("chat"),
        ),
    ] {
        println!("| {name} | {:.0} | {:.1} |", c.delta_bytes, c.us_per_call);
    }
    delta_row::report();
}
