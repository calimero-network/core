//! Engine benchmarks for `calimero-search`.
//!
//! Exercises the code the node runs — `ContextIndex` over a tantivy
//! `RamDirectory`, which is what `calimero-node`'s `services::Search`
//! opens every app's index over — against deterministic chat corpora, and
//! prints a markdown report. tantivy's stock `MmapDirectory` runs beside it as
//! the on-disk baseline. The `gas` section fits the host time of a query to the
//! work it reports, which is what the runtime's `search_query` gas constants
//! are derived from.
//!
//! ```text
//! cargo run --release -p search-bench -- [--sizes 10000,100000] [--only size|memory|cross|gas] [--docs] [--iters N] [--out FILE]
//! ```

mod alloc;
mod corpus;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use calimero_primitives::search::{
    SearchDoc, SearchFilter, SearchMode, SearchOrder, SearchRequest,
};
use calimero_search::index::{ContextIndex, SchemaOptions, WRITER_MEMORY};
use calimero_search::tokenize;
use corpus::Message;
use eyre::{bail, eyre, Result as EyreResult};
use tantivy::directory::{MmapDirectory, RamDirectory};
use tantivy::Directory;

#[global_allocator]
static ALLOC: alloc::Counting = alloc::Counting;

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
    /// What the node opens every index over.
    Ram,
    /// tantivy's stock on-disk directory, the baseline.
    Mmap,
}

/// Where an index's bytes are, to measure them.
enum Backing {
    Ram(RamDirectory),
    Mmap(PathBuf),
}

impl Backing {
    fn bytes(&self) -> u64 {
        match self {
            Self::Ram(dir) => dir.total_mem_usage() as u64,
            Self::Mmap(path) => du(path),
        }
    }
}

impl Drop for Backing {
    fn drop(&mut self) {
        if let Self::Mmap(path) = self {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

struct Built {
    index: ContextIndex,
    backing: Backing,
    secs: f64,
}

fn open_dir(kind: DirKind, path: &Path) -> EyreResult<(Box<dyn Directory>, Backing)> {
    Ok(match kind {
        DirKind::Ram => {
            let dir = RamDirectory::create();
            (Box::new(dir.clone()), Backing::Ram(dir))
        }
        DirKind::Mmap => {
            let _ = std::fs::remove_dir_all(path);
            std::fs::create_dir_all(path)?;
            (
                Box::new(MmapDirectory::open(path)?),
                Backing::Mmap(path.to_path_buf()),
            )
        }
    })
}

fn build(
    kind: DirKind,
    path: &Path,
    msgs: &[Message],
    opts: SchemaOptions,
    batch: usize,
) -> EyreResult<Built> {
    let (dir, backing) = open_dir(kind, path)?;
    let (Some(index), _) = ContextIndex::open(dir, &corpus::schema(), opts)? else {
        bail!("fresh index did not open");
    };
    let t = Instant::now();
    for (i, chunk) in msgs.chunks(batch).enumerate() {
        let docs: Vec<SearchDoc> = chunk.iter().map(corpus::document).collect();
        let _ = index.apply(docs.iter().map(|d| (&d.id, Some(d))))?;
        index.commit(i as u64 + 1, [0; 32])?;
    }
    index.close_writer()?;
    Ok(Built {
        index,
        backing,
        secs: t.elapsed().as_secs_f64(),
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
        order: SearchOrder::Relevance,
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
    // Words the corpus never produces: the dictionary lookup alone.
    let none = (0..20)
        .map(|i| req(format!("qqx{i}qq"), SearchMode::Words, vec![]))
        .collect();
    let mut sets = vec![
        QuerySet {
            label: "rare word",
            requests: rare,
        },
        QuerySet {
            label: "no match",
            requests: none,
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
    p95: Duration,
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
        p95: pct(&times, 0.95),
        p99: pct(&times, 0.99),
        avg_total: totals as f64 / set.requests.len() as f64,
    })
}

/// The baseline an app without search ships: lowercase every message, test
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
    rep.line("| schema | directory | build (1k-doc commits) | per message | index bytes | per message | index / raw text |");
    rep.line("|---|---|---|---|---|---|---|");

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
        for kind in [DirKind::Ram, DirKind::Mmap] {
            let path = root.join(format!("size-{n}-{label}-{kind:?}").replace(' ', ""));
            let built = build(kind, &path, &msgs, opts, 1_000)?;
            let bytes = built.backing.bytes();
            rep.line(format!(
                "| {label} | {kind:?} | {:.2} s | {} | {} | {} B | {:.1}× |",
                built.secs,
                us(Duration::from_secs_f64(built.secs / n as f64)),
                mib(bytes),
                bytes / n as u64,
                bytes as f64 / text_bytes as f64,
            ));
            if label == "words + trigrams" {
                let key = match kind {
                    DirKind::Ram => "ram",
                    DirKind::Mmap => "mmap",
                };
                let _ = keep.insert(key, built);
            }
        }
    }

    let ram = keep.remove("ram").ok_or_else(|| eyre!("no ram build"))?;
    let mmap = keep.remove("mmap").ok_or_else(|| eyre!("no mmap build"))?;

    let (ok, all) = verify(&ram.index, &msgs)?;
    rep.line(format!(
        "\nCorrectness: {ok}/{all} sampled queries (word, 4-char prefix, 4-char substring, accent/CJK) return exactly the match count a fold-aware scan finds.\n"
    ));
    if ok != all {
        bail!("the index disagrees with a scan on {} queries", all - ok);
    }

    rep.line(format!(
        "Query latency, words + trigrams index, warm, {iters} timed queries per row (single thread):\n"
    ));
    rep.line(
        "| query | avg matches | RamDirectory p50 | p95 | p99 | MmapDirectory p50 | p95 | p99 |",
    );
    rep.line("|---|---|---|---|---|---|---|---|");
    for set in query_sets(true) {
        let a = run_queries(&ram.index, &set, iters)?;
        let b = run_queries(&mmap.index, &set, iters)?;
        rep.line(format!(
            "| {} | {:.0} | {} | {} | {} | {} | {} | {} |",
            set.label,
            a.avg_total,
            us(a.p50),
            us(a.p95),
            us(a.p99),
            us(b.p50),
            us(b.p95),
            us(b.p99)
        ));
    }

    // One message, committed on its own: what a view pays to catch the index
    // up after a single write (apply the touched entity, commit, reload).
    let mut times = Vec::new();
    for i in 0..50 {
        let m = corpus::messages(1, 1_000 + i).remove(0);
        let d = corpus::document(&m);
        let t = Instant::now();
        let _ = ram.index.apply([(&d.id, Some(&d))])?;
        ram.index.commit(1_000_000 + i, [0; 32])?;
        times.push(t.elapsed());
    }
    ram.index.close_writer()?;
    times.sort();
    rep.line(format!(
        "\nSingle message + its own commit at {n} docs (RamDirectory, what a view pays to catch up on one write): p50 {}, p99 {}.\n",
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
        "Native linear scan over {n} in-memory strings (lowercase + contains + sort; a LOWER bound for an app scanning its own state — no WASM, no storage reads): rare p50 {} / p99 {}, common p50 {} / p99 {}, infix p50 {} / p99 {}.\n",
        us(r50), us(r99), us(c50), us(c99), us(i50), us(i99)
    ));
    Ok(())
}

fn ram_index(dir: &RamDirectory) -> EyreResult<ContextIndex> {
    let (Some(index), _) = ContextIndex::open(
        Box::new(dir.clone()),
        &corpus::schema(),
        SchemaOptions::default(),
    )?
    else {
        bail!("fresh index did not open");
    };
    Ok(index)
}

fn fill(index: &ContextIndex, msgs: &[Message], batch: usize) -> EyreResult<()> {
    for chunk in msgs.chunks(batch) {
        let docs: Vec<SearchDoc> = chunk.iter().map(corpus::document).collect();
        let _ = index.apply(docs.iter().map(|d| (&d.id, Some(d))))?;
        index.commit(1, [0; 32])?;
    }
    index.close_writer()
}

fn section_memory(rep: &mut Report) -> EyreResult<()> {
    rep.line("\n### Memory (Rust heap, counting allocator; the RamDirectory's bytes included)\n");
    let msgs = corpus::messages(10_000, 3);

    let base = alloc::reset_peak();
    let dir = RamDirectory::create();
    let index = ram_index(&dir)?;
    let with_reader = alloc::live();
    let d = corpus::document(&msgs[0]);
    let _ = index.apply([(&d.id, Some(&d))])?;
    let with_writer = alloc::live();
    fill(&index, &msgs, 1_000)?;
    let peak = alloc::peak();
    let after_close = alloc::live();
    for set in query_sets(false) {
        for q in &set.requests {
            let _ = index.search(q)?;
        }
    }
    let after_queries = alloc::live();
    let dir_bytes = dir.total_mem_usage() as u64;
    drop(index);
    drop(dir);

    // Ten apps' indexes of 10k documents each, writers closed, open for query.
    let before_ten = alloc::live();
    let mut ten = Vec::new();
    for c in 0..10_u64 {
        let dir = RamDirectory::create();
        let index = ram_index(&dir)?;
        fill(&index, &corpus::messages(10_000, c + 10), 2_000)?;
        let _ = index.search(&req("needle", SearchMode::Words, vec![]))?;
        ten.push(index);
    }
    let ten_open = alloc::live();
    drop(ten);

    rep.line("| state | heap |");
    rep.line("|---|---|");
    rep.line(format!(
        "| one empty index open, reader only | {} |",
        mib(with_reader.saturating_sub(base) as u64)
    ));
    rep.line(format!(
        "| + writer open (1 thread, {} MB arena budget) | {} |",
        WRITER_MEMORY / 1_000_000,
        mib(with_writer.saturating_sub(base) as u64)
    ));
    rep.line(format!(
        "| peak while indexing 10k docs in 1k-doc commits | {} |",
        mib(peak.saturating_sub(base) as u64)
    ));
    rep.line(format!(
        "| writer closed (the directory holds {}) | {} |",
        mib(dir_bytes),
        mib(after_close.saturating_sub(base) as u64)
    ));
    rep.line(format!(
        "| after every query kind ran | {} |",
        mib(after_queries.saturating_sub(base) as u64)
    ));
    rep.line(format!(
        "| 10 apps × 10k docs open for query (per app) | {} ({} each) |",
        mib(ten_open.saturating_sub(before_ten) as u64),
        mib((ten_open.saturating_sub(before_ten) / 10) as u64)
    ));
    rep.line("\nThe node keeps every index in memory (rebuilt from state after a restart), so the per-app figure is the resident cost of one searchable app of this size.\n");
    Ok(())
}

fn section_cross_app(rep: &mut Report, iters: usize) -> EyreResult<()> {
    rep.line("\n### Across apps: 10 indexes × 10k messages\n");
    let t = Instant::now();
    let mut indexes = Vec::new();
    for i in 0..10_u64 {
        let dir = RamDirectory::create();
        let index = ram_index(&dir)?;
        fill(&index, &corpus::messages(10_000, 200 + i), 2_000)?;
        indexes.push(index);
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
            for (k, index) in indexes.iter().enumerate() {
                hits.extend(index.search(q)?.hits.into_iter().map(|h| (k, h)));
            }
            hits.sort_by(|a, b| b.1.score.total_cmp(&a.1.score));
            hits.truncate(20);
            seq.push(t.elapsed());

            let t = Instant::now();
            let mut hits: Vec<_> = std::thread::scope(|s| {
                let handles: Vec<_> = indexes
                    .iter()
                    .enumerate()
                    .map(|(k, index)| s.spawn(move || index.search(q).map(|r| (k, r.hits))))
                    .collect();
                handles
                    .into_iter()
                    .filter_map(|h| h.join().ok().and_then(Result::ok))
                    .flat_map(|(k, hits)| hits.into_iter().map(move |h| (k, h)))
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
    rep.line("\nA view searches only its own app's index; this is what a client merging several apps' answers would pay. The merge sorts by raw BM25, which is only roughly comparable across indexes (each has its own IDF). Threads are spawned per query; a pool would be cheaper.\n");
    Ok(())
}

fn section_docs(rep: &mut Report, root: &Path, iters: usize) -> EyreResult<()> {
    rep.line("\n### Docs: 10k documents × 50 blocks (one search document per block)\n");
    let msgs = corpus::blocks(10_000, 50, 9);
    let text: usize = msgs.iter().map(|m| m.text.len()).sum();
    let built = build(
        DirKind::Ram,
        &root.join("docs"),
        &msgs,
        SchemaOptions::default(),
        5_000,
    )?;
    let bytes = built.backing.bytes();
    rep.line(format!(
        "{} blocks, {} of text; built (words + trigrams, RamDirectory, 5k-block commits) in {:.1} s = {} per block; {} in memory ({} B per block).\n",
        msgs.len(),
        mib(text as u64),
        built.secs,
        us(Duration::from_secs_f64(built.secs / msgs.len() as f64)),
        mib(bytes),
        bytes / msgs.len() as u64
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
    Ok(())
}

/// One query's host time and the work it reported.
struct GasSample {
    matched: f64,
    hits: f64,
    bytes: f64,
    ns: f64,
}

/// Least squares for `ns ≈ c0 + c1·matched + c2·hits + c3·bytes`, by the
/// normal equations.
fn fit(samples: &[GasSample]) -> EyreResult<[f64; 4]> {
    let mut a = [[0.0_f64; 5]; 4];
    for s in samples {
        let x = [1.0, s.matched, s.hits, s.bytes];
        for i in 0..4 {
            for j in 0..4 {
                a[i][j] += x[i] * x[j];
            }
            a[i][4] += x[i] * s.ns;
        }
    }
    for col in 0..4 {
        let pivot = (col..4)
            .max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))
            .unwrap_or(col);
        a.swap(col, pivot);
        if a[col][col].abs() < f64::EPSILON {
            bail!("the gas fit is singular: the queries do not vary enough");
        }
        for row in 0..4 {
            if row != col {
                let f = a[row][col] / a[col][col];
                let pivot_row = a[col];
                for (x, p) in a[row].iter_mut().zip(pivot_row).skip(col) {
                    *x -= f * p;
                }
            }
        }
    }
    Ok([
        a[0][4] / a[0][0],
        a[1][4] / a[1][1],
        a[2][4] / a[2][2],
        a[3][4] / a[3][3],
    ])
}

/// The host side of `search_query` (the query, and the borsh response the
/// host writes back), timed against what it reports: documents matched, hits
/// returned, response bytes. The fit is what the runtime's gas constants are
/// set from, at the guest's own rate of gas per nanosecond.
fn section_gas(rep: &mut Report, root: &Path, sizes: &[usize]) -> EyreResult<()> {
    rep.line("\n### The host time of a query, for search gas\n");
    let mut samples = Vec::new();
    rep.line("| messages | query | matched | hits | response bytes | host p50 |");
    rep.line("|---|---|---|---|---|---|");
    for &n in sizes {
        let msgs = corpus::messages(n, 11);
        let built = build(
            DirKind::Ram,
            &root.join(format!("gas-{n}")),
            &msgs,
            SchemaOptions::default(),
            1_000,
        )?;
        for set in query_sets(true) {
            let mut times = Vec::new();
            let (mut matched, mut hits, mut bytes) = (0, 0, 0);
            for round in 0..5 {
                for q in &set.requests {
                    let t = Instant::now();
                    let response = built.index.search(q)?;
                    let encoded = borsh::to_vec(&response)?;
                    let ns = t.elapsed();
                    // The first round warms the caches, as a live index is.
                    if round == 0 {
                        continue;
                    }
                    times.push(ns);
                    samples.push(GasSample {
                        matched: response.total as f64,
                        hits: response.hits.len() as f64,
                        bytes: encoded.len() as f64,
                        ns: ns.as_secs_f64() * 1e9,
                    });
                    matched += response.total;
                    hits += response.hits.len();
                    bytes += encoded.len();
                }
            }
            times.sort();
            let k = times.len().max(1);
            rep.line(format!(
                "| {n} | {} | {} | {} | {} | {} |",
                set.label,
                matched / k as u64,
                hits / k,
                bytes / k,
                us(pct(&times, 0.5))
            ));
        }
    }
    let [base, per_match, per_hit, per_byte] = fit(&samples)?;
    rep.line(format!(
        "\nLeast-squares fit over {} queries: host time ≈ {:.1} µs + {:.1} ns per matched document + {:.2} µs per hit + {:.2} ns per response byte.",
        samples.len(),
        base / 1e3,
        per_match,
        per_hit / 1e3,
        per_byte
    ));
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
            .join("search-bench")
            .display()
            .to_string()
    }));
    std::fs::create_dir_all(&root)?;
    let iters = arg("--iters").map_or(Ok(400), |s| s.parse())?;

    let mut rep = Report::default();
    rep.line("## Engine benchmark (tools/search-bench)\n");
    rep.line(format!(
        "Release build, one thread per query unless stated, on this machine ({} cores). Per-message times are wall-clock time of the single indexing thread (tantivy's merge thread runs beside it).",
        std::thread::available_parallelism().map_or(0, |n| n.get())
    ));
    let only = arg("--only");
    let run = |name: &str| only.as_deref().is_none_or(|o| o == name);
    if run("size") {
        for n in &sizes {
            section_size_and_queries(&mut rep, &root, *n, iters)?;
        }
    }
    if run("memory") {
        section_memory(&mut rep)?;
    }
    if run("cross") {
        section_cross_app(&mut rep, iters.min(200))?;
    }
    if run("gas") {
        section_gas(&mut rep, &root, &sizes)?;
    }
    if args.iter().any(|a| a == "--docs") {
        section_docs(&mut rep, &root, iters.min(200))?;
    }
    let peak = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|l| l.starts_with("VmHWM:"))
                .map(|l| l.trim_start_matches("VmHWM:").trim().to_owned())
        })
        .unwrap_or_else(|| "unavailable (no /proc)".to_owned());
    rep.line(format!(
        "\nPeak RSS of the whole run (every section): {peak}.\n"
    ));
    if let Some(out) = arg("--out") {
        std::fs::write(out, &rep.0)?;
    }
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
