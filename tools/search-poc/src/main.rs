//! Benchmark harness for the calimero-search PoC.
//!
//! Exercises the same code the node runs — `RocksDirectory` over a real
//! RocksDB `Store`, `ContextIndex`, `SearchService` with the dirty log and the
//! indexer — against deterministic chat corpora, and prints a markdown report.
//! tantivy's stock `MmapDirectory` runs beside it as the baseline.
//!
//! ```text
//! cargo run --release -p search-poc -- [--sizes 10000,100000] [--docs] [--out FILE]
//! ```

mod alloc;
mod corpus;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use calimero_primitives::search::{
    ExtractResponse, ScanResponse, SearchDoc, SearchFilter, SearchIndexSchema, SearchMode,
    SearchRequest,
};
use calimero_search::directory::{ChunkCache, RocksDirectory};
use calimero_search::index::{ContextIndex, SchemaOptions};
use calimero_search::{dirty, tokenize, ContextKey, Extractor, SearchConfig, SearchService};
use calimero_store::config::StoreConfig;
use calimero_store::db::Column;
use calimero_store::slice::Slice;
use calimero_store::tx::Transaction;
use calimero_store::Store;
use calimero_store_rocksdb::RocksDB;
use camino::Utf8PathBuf;
use corpus::Message;
use eyre::{bail, Result as EyreResult};
use tantivy::directory::MmapDirectory;
use tantivy::Directory;

#[global_allocator]
static ALLOC: alloc::Counting = alloc::Counting;

const CTX: ContextKey = [7; 32];

/// The report, printed as it grows.
#[derive(Default)]
struct Report(String);

impl Report {
    fn line(&mut self, s: impl AsRef<str>) {
        println!("{}", s.as_ref());
        self.0.push_str(s.as_ref());
        self.0.push('\n');
    }
}

fn open_store(path: &Path) -> EyreResult<Store> {
    let path = Utf8PathBuf::from_path_buf(path.to_path_buf())
        .map_err(|p| eyre::eyre!("non-utf8 path {p:?}"))?;
    Store::open::<RocksDB>(&StoreConfig::new(path))
}

fn du(path: &Path) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|entries| entries.flatten().map(|e| du(&e.path())).sum())
        .unwrap_or(0)
}

fn pct(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

fn us(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us >= 10_000.0 {
        format!("{:.1} ms", us / 1000.0)
    } else if us >= 100.0 {
        format!("{us:.0} µs")
    } else {
        format!("{us:.1} µs")
    }
}

fn mib(b: u64) -> String {
    format!("{:.2} MiB", b as f64 / (1024.0 * 1024.0))
}

#[derive(Clone, Copy, Debug)]
enum DirKind {
    Rocks,
    Mmap,
}

/// A directory, and for the store-backed one the store and handle behind it.
type OpenedDir = (Box<dyn Directory>, Option<(Store, RocksDirectory)>);

struct Built {
    index: ContextIndex,
    rocks: Option<(Store, RocksDirectory)>,
    path: PathBuf,
    secs: f64,
}

fn open_dir(kind: DirKind, path: &Path, cache: &Arc<ChunkCache>) -> EyreResult<OpenedDir> {
    std::fs::create_dir_all(path)?;
    Ok(match kind {
        DirKind::Rocks => {
            let store = open_store(path)?;
            let dir = RocksDirectory::new(store.clone(), &CTX, "messages", Arc::clone(cache));
            (Box::new(dir.clone()), Some((store, dir)))
        }
        DirKind::Mmap => (Box::new(MmapDirectory::open(path)?), None),
    })
}

fn build(
    kind: DirKind,
    path: &Path,
    msgs: &[Message],
    opts: SchemaOptions,
    batch: usize,
    cache: &Arc<ChunkCache>,
) -> EyreResult<Built> {
    let _ = std::fs::remove_dir_all(path);
    let (dir, rocks) = open_dir(kind, path, cache)?;
    let (Some(index), _) = ContextIndex::open(dir, &corpus::schema(), opts)? else {
        bail!("fresh index did not open");
    };
    let t = Instant::now();
    for (i, chunk) in msgs.chunks(batch).enumerate() {
        let docs: Vec<SearchDoc> = chunk.iter().map(corpus::document).collect();
        let _ = index.apply(docs.iter().map(|d| (&d.id, Some(d))))?;
        index.commit(i as u64 + 1)?;
    }
    index.close_writer()?;
    let secs = t.elapsed().as_secs_f64();
    if let Some((store, _)) = &rocks {
        store.flush()?;
    }
    Ok(Built {
        index,
        rocks,
        path: path.to_path_buf(),
        secs,
    })
}

struct QuerySet {
    label: &'static str,
    requests: Vec<SearchRequest>,
}

fn req(query: impl Into<String>, mode: SearchMode, filters: Vec<SearchFilter>) -> SearchRequest {
    SearchRequest {
        index: "messages".to_owned(),
        query: query.into(),
        mode,
        filters,
        cursor: 0,
        limit: 20,
    }
}

fn query_sets(full: bool) -> Vec<QuerySet> {
    let mut r = corpus::Rng(42);
    let mut rare = vec![req("zebrafish", SearchMode::Words, vec![])];
    for _ in 0..19 {
        rare.push(req(
            corpus::word(12_000 + r.below(8_000)),
            SearchMode::Words,
            vec![],
        ));
    }
    let common = (0..3)
        .map(|i| req(corpus::word(i), SearchMode::Words, vec![]))
        .collect();
    let mut prefix = vec![req("needl", SearchMode::Prefix, vec![])];
    for _ in 0..19 {
        let w = corpus::word(100 + r.below(1_900));
        prefix.push(req(&w[..4], SearchMode::Prefix, vec![]));
    }
    let mut infix = vec![req("eedl", SearchMode::Substring, vec![])];
    for _ in 0..19 {
        let w = corpus::word(300 + r.below(3_000));
        infix.push(req(&w[1..5], SearchMode::Substring, vec![]));
    }
    let infix3 = (0..20)
        .map(|_| {
            let w = corpus::word(300 + r.below(3_000));
            req(&w[1..4], SearchMode::Substring, vec![])
        })
        .collect();
    let mut and = Vec::new();
    for i in 0..5 {
        and.push(req(
            format!("needle {}", corpus::word(i)),
            SearchMode::Words,
            vec![],
        ));
    }
    for _ in 0..15 {
        and.push(req(
            format!(
                "{} {}",
                corpus::word(r.below(50)),
                corpus::word(50 + r.below(500))
            ),
            SearchMode::Words,
            vec![],
        ));
    }
    let facet = (0..20)
        .map(|i| {
            req(
                corpus::word(i % 5),
                SearchMode::Words,
                vec![SearchFilter::Eq {
                    field: "sender".to_owned(),
                    value: corpus::sender(i % 5),
                }],
            )
        })
        .collect();
    let fuzzy = (0..20)
        .map(|_| {
            let mut w: Vec<char> = corpus::word(100 + r.below(1_900)).chars().collect();
            let at = 1 + r.below(w.len() - 1);
            w[at] = if w[at] == 'x' { 'q' } else { 'x' };
            req(w.into_iter().collect::<String>(), SearchMode::Fuzzy, vec![])
        })
        .collect();
    let mut sets = vec![
        QuerySet {
            label: "rare word",
            requests: rare,
        },
        QuerySet {
            label: "common word, top-20",
            requests: common,
        },
        QuerySet {
            label: "prefix (4 chars)",
            requests: prefix,
        },
        QuerySet {
            label: "infix substring (4 chars)",
            requests: infix,
        },
        QuerySet {
            label: "infix substring (3 chars)",
            requests: infix3,
        },
        QuerySet {
            label: "two-term AND",
            requests: and,
        },
        QuerySet {
            label: "sender facet + word",
            requests: facet,
        },
        QuerySet {
            label: "fuzzy (distance 1)",
            requests: fuzzy,
        },
    ];
    if !full {
        sets.retain(|s| !s.label.starts_with("infix substring (3"));
    }
    sets
}

struct QueryStat {
    p50: Duration,
    p99: Duration,
    avg_total: f64,
}

fn run_queries(index: &ContextIndex, set: &QuerySet, iters: usize) -> EyreResult<QueryStat> {
    // One warm-up pass, then timed rounds over the whole set.
    let mut totals = 0_u64;
    for q in &set.requests {
        totals += index.search(q)?.total;
    }
    let mut times = Vec::with_capacity(iters);
    for i in 0..iters {
        let q = &set.requests[i % set.requests.len()];
        let t = Instant::now();
        let res = index.search(q)?;
        times.push(t.elapsed());
        std::hint::black_box(res);
    }
    times.sort();
    Ok(QueryStat {
        p50: pct(&times, 0.5),
        p99: pct(&times, 0.99),
        avg_total: totals as f64 / set.requests.len() as f64,
    })
}

/// The baseline every chat app ships today: lowercase every message, test
/// `contains`, sort the matches newest first, keep 20.
fn scan(msgs: &[Message], term: &str) -> usize {
    let term = term.to_lowercase();
    let mut hits: Vec<&Message> = msgs
        .iter()
        .filter(|m| m.text.to_lowercase().contains(&term))
        .collect();
    hits.sort_by_key(|m| std::cmp::Reverse(m.ts));
    let n = hits.len();
    hits.truncate(20);
    std::hint::black_box(hits);
    n
}

fn scan_stats(msgs: &[Message], terms: &[String], iters: usize) -> (Duration, Duration) {
    let mut times = Vec::new();
    for i in 0..iters {
        let t = Instant::now();
        let _ = scan(msgs, &terms[i % terms.len()]);
        times.push(t.elapsed());
    }
    times.sort();
    (pct(&times, 0.5), pct(&times, 0.99))
}

/// How often the index's answer equals a scan's, per query kind.
fn verify(index: &ContextIndex, msgs: &[Message]) -> EyreResult<(usize, usize)> {
    let mut ok = 0;
    let mut all = 0;
    let folded: Vec<(Vec<String>, String)> = msgs
        .iter()
        .map(|m| {
            (
                tokenize::words(&m.text)
                    .into_iter()
                    .map(|t| t.text)
                    .collect(),
                tokenize::trigram_text(&m.text).into_iter().collect(),
            )
        })
        .collect();
    let mut r = corpus::Rng(7);
    for _ in 0..40 {
        let w = corpus::word(r.zipf(corpus::VOCAB));
        let want = folded.iter().filter(|(ws, _)| ws.contains(&w)).count() as u64;
        all += 1;
        ok += usize::from(index.search(&req(&w, SearchMode::Words, vec![]))?.total == want);

        let sub: String = w.chars().skip(1).take(4).collect();
        if sub.chars().count() >= 3 {
            let want = folded.iter().filter(|(_, t)| t.contains(&sub)).count() as u64;
            all += 1;
            ok += usize::from(
                index
                    .search(&req(&sub, SearchMode::Substring, vec![]))?
                    .total
                    == want,
            );
        }
        let pre: String = w.chars().take(4).collect();
        let want = folded
            .iter()
            .filter(|(ws, _)| ws.iter().any(|t| t.starts_with(&pre)))
            .count() as u64;
        all += 1;
        ok += usize::from(index.search(&req(&pre, SearchMode::Prefix, vec![]))?.total == want);
    }
    for (q, needle) in [
        ("Cafe", "cafe"),
        ("naive", "naive"),
        ("北京", "北京"),
        ("RÉSUMÉ", "resume"),
    ] {
        let want = folded
            .iter()
            .filter(|(ws, _)| ws.iter().any(|t| t == needle))
            .count() as u64;
        all += 1;
        ok += usize::from(index.search(&req(q, SearchMode::Words, vec![]))?.total == want);
    }
    Ok((ok, all))
}

fn section_size_and_queries(
    rep: &mut Report,
    root: &Path,
    n: usize,
    iters: usize,
) -> EyreResult<()> {
    let msgs = corpus::messages(n, 1);
    let text_bytes: usize = msgs.iter().map(|m| m.text.len()).sum();
    rep.line(format!("\n### {n} chat messages\n"));
    rep.line(format!(
        "Corpus: {n} messages, {:.1} words and {} bytes of text per message on average.\n",
        msgs.iter()
            .map(|m| m.text.split_whitespace().count())
            .sum::<usize>() as f64
            / n as f64,
        text_bytes / n
    ));
    rep.line("| schema | directory | build (1k-doc commits) | per message | store rows written | bytes written | live index bytes | on disk | per message on disk |");
    rep.line("|---|---|---|---|---|---|---|---|---|");

    let cache = ChunkCache::new(32 << 20);
    let mut keep: HashMap<&'static str, Built> = HashMap::new();
    for (label, opts) in [
        (
            "words",
            SchemaOptions {
                store_text: true,
                trigrams: false,
            },
        ),
        ("words + trigrams", SchemaOptions::default()),
    ] {
        for kind in [DirKind::Rocks, DirKind::Mmap] {
            let path = root.join(format!("size-{n}-{label}-{kind:?}").replace(' ', ""));
            let built = build(kind, &path, &msgs, opts, 1_000, &cache)?;
            let disk = du(&path);
            let (rows_w, bytes_w, live) = match &built.rocks {
                Some((_, dir)) => {
                    let (rows, bytes, _, _) = dir.stats().snapshot();
                    let (_, live) = dir.stored_bytes()?;
                    (rows.to_string(), mib(bytes), mib(live))
                }
                None => ("—".to_owned(), "—".to_owned(), "—".to_owned()),
            };
            rep.line(format!(
                "| {label} | {kind:?} | {:.2} s | {} | {rows_w} | {bytes_w} | {live} | {} | {} B |",
                built.secs,
                us(Duration::from_secs_f64(built.secs / n as f64)),
                mib(disk),
                disk / n as u64,
            ));
            if label == "words + trigrams" {
                let _ = keep.insert(
                    if matches!(kind, DirKind::Rocks) {
                        "rocks"
                    } else {
                        "mmap"
                    },
                    built,
                );
            }
        }
    }

    let rocks = keep
        .remove("rocks")
        .ok_or_else(|| eyre::eyre!("no rocks build"))?;
    let mmap = keep
        .remove("mmap")
        .ok_or_else(|| eyre::eyre!("no mmap build"))?;

    let (ok, all) = verify(&rocks.index, &msgs)?;
    rep.line(format!(
        "\nCorrectness: {ok}/{all} sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.\n"
    ));

    rep.line(format!("Query latency, words + trigrams index, warm, {iters} timed queries per row (single thread):\n"));
    rep.line("| query | avg matches | RocksDirectory p50 | p99 | MmapDirectory p50 | p99 |");
    rep.line("|---|---|---|---|---|---|");
    for set in query_sets(true) {
        let a = run_queries(&rocks.index, &set, iters)?;
        let b = run_queries(&mmap.index, &set, iters)?;
        rep.line(format!(
            "| {} | {:.0} | {} | {} | {} | {} |",
            set.label,
            a.avg_total,
            us(a.p50),
            us(a.p99),
            us(b.p50),
            us(b.p99)
        ));
    }

    // Cold: a fresh chunk cache and a freshly opened reader, first query only.
    if let Some((store, _)) = &rocks.rocks {
        let mut cold = Vec::new();
        for q in ["zebrafish", "needle", &corpus::word(0)] {
            let dir =
                RocksDirectory::new(store.clone(), &CTX, "messages", ChunkCache::new(32 << 20));
            let t = Instant::now();
            let Some(index) = ContextIndex::open_existing(Box::new(dir.clone()))? else {
                bail!("reopen failed");
            };
            let open = t.elapsed();
            let t = Instant::now();
            let _ = index.search(&req(q, SearchMode::Words, vec![]))?;
            let first = t.elapsed();
            let (_, _, reads, hits) = dir.stats().snapshot();
            cold.push(format!(
                "`{q}`: open {} + first query {} ({reads} chunk reads, {hits} cache hits)",
                us(open),
                us(first)
            ));
        }
        rep.line(format!(
            "\nCold start (RocksDirectory, empty chunk cache): {}.\n",
            cold.join("; ")
        ));
    }

    // One message, committed on its own: the freshness-optimal extreme.
    let mut times = Vec::new();
    for i in 0..50 {
        let m = corpus::messages(1, 1_000 + i).remove(0);
        let d = corpus::document(&m);
        let t = Instant::now();
        let _ = rocks.index.apply([(&d.id, Some(&d))])?;
        rocks.index.commit(1_000_000 + i)?;
        times.push(t.elapsed());
    }
    rocks.index.close_writer()?;
    times.sort();
    rep.line(format!(
        "Single message + its own commit at {n} docs (RocksDirectory): p50 {}, p99 {} — what batching over ~250 ms avoids.\n",
        us(pct(&times, 0.5)),
        us(pct(&times, 0.99))
    ));

    // Baseline: the linear scan.
    let rare: Vec<String> = vec!["zebrafish".into()];
    let common: Vec<String> = vec![corpus::word(0)];
    let infix: Vec<String> = vec!["eedl".into()];
    let scan_iters = if n > 50_000 { 20 } else { 100 };
    let (r50, r99) = scan_stats(&msgs, &rare, scan_iters);
    let (c50, c99) = scan_stats(&msgs, &common, scan_iters);
    let (i50, i99) = scan_stats(&msgs, &infix, scan_iters);
    rep.line(format!(
        "Native linear scan over {n} in-memory strings (lowercase + contains + sort, the mero-chat `search_all_messages` semantics, a LOWER bound — no WASM, no storage reads): rare p50 {} / p99 {}, common p50 {} / p99 {}, infix p50 {} / p99 {}.\n",
        us(r50), us(r99), us(c50), us(c99), us(i50), us(i99)
    ));
    let _ = std::fs::remove_dir_all(&rocks.path);
    let _ = std::fs::remove_dir_all(&mmap.path);
    for built in keep.into_values() {
        let _ = std::fs::remove_dir_all(&built.path);
    }
    for entry in std::fs::read_dir(root)?.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(&format!("size-{n}-"))
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

fn section_memory(rep: &mut Report, root: &Path) -> EyreResult<()> {
    rep.line("\n### Memory (Rust heap, counting allocator)\n");
    let path = root.join("memory");
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path)?;
    let store = open_store(&path)?;
    let service = SearchService::new(store.clone(), SearchConfig::default());
    let msgs = corpus::messages(10_000, 3);

    let base = alloc::reset_peak();
    let (index, _) = service.open_index(&CTX, &corpus::schema())?;
    let with_reader = alloc::live();
    let d = corpus::document(&msgs[0]);
    let _ = index.apply([(&d.id, Some(&d))])?;
    let with_writer = alloc::live();
    for chunk in msgs.chunks(1_000) {
        let docs: Vec<SearchDoc> = chunk.iter().map(corpus::document).collect();
        let _ = index.apply(docs.iter().map(|d| (&d.id, Some(d))))?;
        index.commit(1)?;
    }
    let peak = alloc::peak();
    index.close_writer()?;
    let after_close = alloc::live();
    for set in query_sets(false) {
        for q in &set.requests {
            let _ = index.search(q)?;
        }
    }
    let after_queries = alloc::live();
    let cache = service.cache().bytes();

    // Nine more contexts, reader only (built, writers closed, then reopened).
    for c in 1..10_u8 {
        let ctx = [c; 32];
        let (index, _) = service.open_index(&ctx, &corpus::schema())?;
        for chunk in corpus::messages(10_000, u64::from(c) + 10).chunks(2_000) {
            let docs: Vec<SearchDoc> = chunk.iter().map(corpus::document).collect();
            let _ = index.apply(docs.iter().map(|d| (&d.id, Some(d))))?;
            index.commit(1)?;
        }
        index.close_writer()?;
    }
    drop(index);
    drop(service);
    let before_open = alloc::live();
    let service = SearchService::new(store, SearchConfig::default());
    for c in 0..10_u8 {
        let ctx = if c == 0 { CTX } else { [c; 32] };
        let _ = service.search(&ctx, &req("needle", SearchMode::Words, vec![]))?;
    }
    let ten_open = alloc::live();

    rep.line("| state | heap |");
    rep.line("|---|---|");
    rep.line(format!(
        "| one index open, reader only (10k docs to come) | {} |",
        mib((with_reader - base) as u64)
    ));
    rep.line(format!(
        "| + writer open (1 thread, {} MB arena budget) | {} |",
        calimero_search::index::WRITER_MEMORY / 1_000_000,
        mib((with_writer - base) as u64)
    ));
    rep.line(format!(
        "| peak while indexing 10k docs in 1k-doc commits | {} |",
        mib((peak - base) as u64)
    ));
    rep.line(format!(
        "| writer closed | {} |",
        mib((after_close.saturating_sub(base)) as u64)
    ));
    rep.line(format!(
        "| after every query kind ran (chunk cache holds {}) | {} |",
        mib(cache as u64),
        mib((after_queries.saturating_sub(base)) as u64)
    ));
    rep.line(format!(
        "| 10 contexts × 10k docs open for query, fresh service (per context) | {} ({} each) |",
        mib((ten_open.saturating_sub(before_open)) as u64),
        mib((ten_open.saturating_sub(before_open) / 10) as u64)
    ));
    rep.line(format!(
        "\nRocksDB's own block cache ({}) is C++-allocated and not counted; it caches the same chunks a second time.\n",
        "the node's DEFAULT_BLOCK_CACHE_SIZE"
    ));
    drop(service);
    let _ = std::fs::remove_dir_all(&path);
    Ok(())
}

fn section_cross_context(rep: &mut Report, root: &Path, iters: usize) -> EyreResult<()> {
    rep.line("\n### Cross-context: 10 contexts × 10k messages\n");
    let path = root.join("cross");
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path)?;
    let store = open_store(&path)?;
    let service = SearchService::new(store, SearchConfig::default());
    let contexts: Vec<ContextKey> = (0..10_u8).map(|c| [c + 100; 32]).collect();
    let t = Instant::now();
    for (i, ctx) in contexts.iter().enumerate() {
        let (index, _) = service.open_index(ctx, &corpus::schema())?;
        for chunk in corpus::messages(10_000, 200 + i as u64).chunks(2_000) {
            let docs: Vec<SearchDoc> = chunk.iter().map(corpus::document).collect();
            let _ = index.apply(docs.iter().map(|d| (&d.id, Some(d))))?;
            index.commit(1)?;
        }
        index.close_writer()?;
    }
    rep.line(format!("Built in {:.1} s.\n", t.elapsed().as_secs_f64()));
    rep.line("| query | sequential p50 | p99 | parallel (10 threads) + merge p50 | p99 |");
    rep.line("|---|---|---|---|---|");
    for set in query_sets(false) {
        let mut seq = Vec::new();
        let mut par = Vec::new();
        for i in 0..iters {
            let q = &set.requests[i % set.requests.len()];
            let t = Instant::now();
            let mut hits = Vec::new();
            for ctx in &contexts {
                hits.extend(service.search(ctx, q)?.hits.into_iter().map(|h| (*ctx, h)));
            }
            hits.sort_by(|a, b| b.1.score.total_cmp(&a.1.score));
            hits.truncate(20);
            seq.push(t.elapsed());

            let t = Instant::now();
            let mut hits: Vec<_> = std::thread::scope(|s| {
                let handles: Vec<_> = contexts
                    .iter()
                    .map(|ctx| {
                        let service = &service;
                        s.spawn(move || service.search(ctx, q).map(|r| (ctx, r.hits)))
                    })
                    .collect();
                handles
                    .into_iter()
                    .filter_map(|h| h.join().ok().and_then(Result::ok))
                    .flat_map(|(ctx, hits)| hits.into_iter().map(move |h| (*ctx, h)))
                    .collect()
            });
            hits.sort_by(|a, b| b.1.score.total_cmp(&a.1.score));
            hits.truncate(20);
            par.push(t.elapsed());
        }
        seq.sort();
        par.sort();
        rep.line(format!(
            "| {} | {} | {} | {} | {} |",
            set.label,
            us(pct(&seq, 0.5)),
            us(pct(&seq, 0.99)),
            us(pct(&par, 0.5)),
            us(pct(&par, 0.99))
        ));
    }
    rep.line("\nThe merge sorts by raw BM25, which is only roughly comparable across contexts (each has its own IDF — by design, see isolation). Threads here are spawned per query; a node would use a pool.\n");
    drop(service);
    let _ = std::fs::remove_dir_all(&path);
    Ok(())
}

/// An app's extractor, over an in-memory "state".
#[derive(Default)]
struct FakeApp {
    state: Mutex<HashMap<[u8; 32], SearchDoc>>,
    extract_calls: AtomicUsize,
    fail_after: AtomicUsize,
}

#[async_trait]
impl Extractor for FakeApp {
    async fn schema(&self, _: ContextKey) -> EyreResult<Option<Vec<SearchIndexSchema>>> {
        Ok(Some(vec![corpus::schema()]))
    }

    async fn extract(
        &self,
        _: ContextKey,
        _: &str,
        ids: Vec<[u8; 32]>,
    ) -> EyreResult<ExtractResponse> {
        let calls = self.extract_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if calls > self.fail_after.load(Ordering::SeqCst) {
            bail!("simulated crash during extraction");
        }
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(ids.iter().map(|id| state.get(id).cloned()).collect())
    }

    async fn scan(
        &self,
        _: ContextKey,
        _: &str,
        offset: u32,
        limit: u32,
    ) -> EyreResult<ScanResponse> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut all: Vec<&SearchDoc> = state.values().collect();
        all.sort_by_key(|d| d.id);
        let docs: Vec<SearchDoc> = all
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect();
        let next = (docs.len() == limit as usize).then_some(offset + limit);
        Ok(ScanResponse { docs, next })
    }
}

/// A committed "execution": the state row and its dirty row in one batch.
fn commit_change(
    store: &Store,
    app: &FakeApp,
    change: Result<SearchDoc, [u8; 32]>,
) -> EyreResult<usize> {
    let mut tx = Transaction::default();
    let id = match &change {
        Ok(doc) => doc.id,
        Err(id) => *id,
    };
    // The "state" half of the batch.
    let mut key = CTX.to_vec();
    key.extend_from_slice(&id);
    match &change {
        Ok(doc) => tx.raw_put(
            Column::State,
            Slice::from(key),
            Slice::from(borsh::to_vec(doc)?),
        ),
        Err(_) => tx.raw_delete(Column::State, Slice::from(key)),
    }
    let (_, size) =
        dirty::stage(&mut tx, &CTX, &[id])?.ok_or_else(|| eyre::eyre!("nothing staged"))?;
    store.apply(&tx)?;
    let mut state = app.state.lock().unwrap_or_else(PoisonError::into_inner);
    match change {
        Ok(doc) => {
            let _ = state.insert(doc.id, doc);
        }
        Err(id) => {
            let _ = state.remove(&id);
        }
    }
    Ok(size)
}

fn section_freshness_and_replay(rep: &mut Report, root: &Path) -> EyreResult<()> {
    rep.line("\n### Freshness and crash-restart replay (SearchService, RocksDB)\n");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    for interval_ms in [250_u64, 50] {
        let path = root.join(format!("fresh-{interval_ms}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)?;
        let store = open_store(&path)?;
        let config = SearchConfig {
            commit_interval: Duration::from_millis(interval_ms),
            ..SearchConfig::default()
        };
        let service = SearchService::new(store.clone(), config);
        let app = Arc::new(FakeApp {
            fail_after: AtomicUsize::new(usize::MAX),
            ..FakeApp::default()
        });
        for m in corpus::messages(10_000, 5) {
            let _ = app
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(m.id, corpus::document(&m));
        }
        let lags = rt.block_on(async {
            let (index, _) = service.open_index(&CTX, &corpus::schema())?;
            let _ = service.index_context(&*app, CTX).await?; // full build
            drop(index);
            let indexer =
                tokio::spawn(Arc::clone(&service).run_indexer(app.clone() as Arc<dyn Extractor>));
            let mut lags = Vec::new();
            for i in 0..40_u32 {
                let token = format!("fresh{i}marker");
                let mut m = corpus::messages(1, 9_000 + u64::from(i)).remove(0);
                m.text = format!("{} {token}", m.text);
                let _ = commit_change(&store, &app, Ok(corpus::document(&m)))?;
                let t = Instant::now();
                service.notify(CTX);
                loop {
                    if service
                        .search(&CTX, &req(&token, SearchMode::Words, vec![]))?
                        .total
                        == 1
                    {
                        break;
                    }
                    if t.elapsed() > Duration::from_secs(10) {
                        bail!("{token} never became searchable");
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                lags.push(t.elapsed());
                // Land the next write at a random phase of the tick.
                tokio::time::sleep(Duration::from_millis(u64::from(i * 37) % interval_ms)).await;
            }
            indexer.abort();
            eyre::Ok(lags)
        })?;
        let mut lags = lags;
        lags.sort();
        rep.line(format!(
            "Apply → searchable with a {interval_ms} ms commit interval (40 writes at random tick phases, 10k-doc index): p50 {}, p99 {}, max {}.",
            us(pct(&lags, 0.5)),
            us(pct(&lags, 0.99)),
            us(*lags.last().unwrap_or(&Duration::ZERO))
        ));
        drop(service);
        let _ = std::fs::remove_dir_all(&path);
    }

    // Crash-restart replay.
    let path = root.join("replay");
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path)?;
    let config = SearchConfig {
        max_rows_per_commit: 1_000,
        extract_batch: 250,
        ..SearchConfig::default()
    };
    let msgs = corpus::messages(5_000, 6);
    let mut expect: HashMap<[u8; 32], String> = HashMap::new();
    let app = FakeApp::default();
    let mut dirty_bytes = 0;
    let (edited, deleted) = {
        let store = open_store(&path)?;
        for m in &msgs {
            dirty_bytes += commit_change(&store, &app, Ok(corpus::document(m)))?;
            let _ = expect.insert(m.id, m.text.clone());
        }
        let mut edited = BTreeSet::new();
        let mut deleted = BTreeSet::new();
        for (i, m) in msgs.iter().enumerate() {
            if i % 10 == 1 {
                let mut e = m.clone();
                e.text = "edited replayed marker".to_owned();
                let _ = commit_change(&store, &app, Ok(corpus::document(&e)))?;
                let _ = expect.insert(m.id, e.text);
                let _ = edited.insert(m.id);
            } else if i % 10 == 2 {
                let _ = commit_change(&store, &app, Err(m.id))?;
                let _ = expect.remove(&m.id);
                let _ = deleted.insert(m.id);
            }
        }
        // First run: dies inside the third extract call (the 2nd commit's batch).
        let service = SearchService::new(store.clone(), config);
        app.fail_after.store(6, Ordering::SeqCst);
        let first = rt.block_on(service.index_context(&app, CTX));
        let committed = service
            .get_index(&CTX, "messages")?
            .map_or(0, |i| i.committed_seq());
        let left = dirty::read_after(&store, &CTX, 0, usize::MAX)?.len();
        rep.line(format!(
            "\nReplay: 5000 posts + 500 edits + 500 deletes, one dirty row each ({} B per row on average). Run 1 crashed mid-pass ({}); it had committed through seq {committed} and left {left} dirty rows.",
            dirty_bytes / msgs.len(),
            first.err().map_or("no error?".to_owned(), |e| e.to_string())
        ));
        drop(service);
        store.flush()?;
        (edited, deleted)
    };
    // Process restart: reopen RocksDB from disk; a lost trim is simulated by
    // re-adding an already-covered row.
    let store = open_store(&path)?;
    let service = SearchService::new(store.clone(), config);
    app.extract_calls.store(0, Ordering::SeqCst);
    app.fail_after.store(usize::MAX, Ordering::SeqCst);
    let t = Instant::now();
    let report = rt.block_on(service.index_context(&app, CTX))?;
    let replay_time = t.elapsed();
    let index = service
        .get_index(&CTX, "messages")?
        .ok_or_else(|| eyre::eyre!("no index"))?;
    let mut tx = Transaction::default();
    let replayed_id = msgs[0].id;
    let _ = dirty::stage(&mut tx, &CTX, &[replayed_id])?;
    store.apply(&tx)?;
    let _ = rt.block_on(service.index_context(&app, CTX))?;

    let docs_ok = index.num_docs() == expect.len() as u64;
    let edited_hits = index.search(&SearchRequest {
        limit: 100,
        ..req("replayed", SearchMode::Words, vec![])
    })?;
    let edits_ok = edited_hits.total == edited.len() as u64;
    let mut deletes_ok = true;
    for id in deleted.iter().take(50) {
        let text = &msgs
            .iter()
            .find(|m| m.id == *id)
            .map(|m| m.text.clone())
            .unwrap_or_default();
        let word = text.split_whitespace().next().unwrap_or("x").to_owned();
        let res = index.search(&SearchRequest {
            limit: 100,
            ..req(word, SearchMode::Words, vec![])
        })?;
        deletes_ok &= res.hits.iter().all(|h| !deleted.contains(&h.id));
    }
    let log_empty = dirty::read_after(&store, &CTX, 0, 10)?.is_empty();
    rep.line(format!(
        "Run 2 (after reopening the store) resumed from the commit payload: {} rows, {} ids, {} docs in {} → documents {} ({} vs {} expected), edits {} , deletes {}, dirty log empty {}. A re-delivered, already-indexed row replays idempotently (count unchanged: {}).\n",
        report.rows,
        report.ids,
        report.docs,
        us(replay_time),
        if docs_ok { "OK" } else { "MISMATCH" },
        index.num_docs(),
        expect.len(),
        if edits_ok { "OK" } else { "MISMATCH" },
        if deletes_ok { "OK" } else { "MISMATCH" },
        log_empty,
        index.num_docs() == expect.len() as u64,
    ));
    if !(docs_ok && edits_ok && deletes_ok && log_empty) {
        bail!("replay produced a wrong index");
    }
    drop(index);
    drop(service);
    let _ = std::fs::remove_dir_all(&path);
    Ok(())
}

fn section_docs(rep: &mut Report, root: &Path, iters: usize) -> EyreResult<()> {
    rep.line("\n### Docs: 10k documents × 50 blocks (one search document per block)\n");
    let msgs = corpus::blocks(10_000, 50, 9);
    let text: usize = msgs.iter().map(|m| m.text.len()).sum();
    let cache = ChunkCache::new(32 << 20);
    let path = root.join("docs");
    let built = build(
        DirKind::Rocks,
        &path,
        &msgs,
        SchemaOptions::default(),
        5_000,
        &cache,
    )?;
    let disk = du(&path);
    rep.line(format!(
        "{} blocks, {} of text; built (words + trigrams, RocksDirectory, 5k-block commits) in {:.1} s = {} per block; {} on disk ({} B per block).\n",
        msgs.len(),
        mib(text as u64),
        built.secs,
        us(Duration::from_secs_f64(built.secs / msgs.len() as f64)),
        mib(disk),
        disk / msgs.len() as u64
    ));
    rep.line("| query | avg matches | p50 | p99 |");
    rep.line("|---|---|---|---|");
    for set in query_sets(false)
        .into_iter()
        .filter(|s| !s.label.starts_with("sender"))
    {
        let s = run_queries(&built.index, &set, iters)?;
        rep.line(format!(
            "| {} | {:.0} | {} | {} |",
            set.label,
            s.avg_total,
            us(s.p50),
            us(s.p99)
        ));
    }
    drop(built);
    let _ = std::fs::remove_dir_all(&path);
    Ok(())
}

fn main() -> EyreResult<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let sizes: Vec<usize> = arg("--sizes")
        .unwrap_or_else(|| "10000,100000".to_owned())
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    let root = PathBuf::from(arg("--tmp").unwrap_or_else(|| {
        std::env::temp_dir()
            .join("search-poc")
            .display()
            .to_string()
    }));
    std::fs::create_dir_all(&root)?;
    let iters = arg("--iters").map_or(Ok(400), |s| s.parse())?;

    let mut rep = Report::default();
    rep.line("## Engine benchmark (tools/search-poc)\n");
    rep.line(format!(
        "Release build, one thread per query unless stated, on this machine ({} cores). Per-message times are wall-clock CPU of the single indexing thread (tantivy's merge thread runs beside it).",
        std::thread::available_parallelism().map_or(0, |n| n.get())
    ));
    for n in &sizes {
        section_size_and_queries(&mut rep, &root, *n, iters)?;
    }
    section_memory(&mut rep, &root)?;
    section_cross_context(&mut rep, &root, iters.min(200))?;
    section_freshness_and_replay(&mut rep, &root)?;
    if args.iter().any(|a| a == "--docs") {
        section_docs(&mut rep, &root, iters.min(200))?;
    }
    if let Some(out) = arg("--out") {
        std::fs::write(out, &rep.0)?;
    }
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
