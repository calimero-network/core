//! The end-to-end benchmarks behind `tools/search-bench/README.md`: a live
//! `ContextManager` on RocksDB running the real `search-chat` wasm. Ignored by
//! default; run in release:
//!
//! ```text
//! SEARCH_BENCH_N=10000 cargo test --release -p calimero-context --lib search_e2e_bench -- --ignored --nocapture
//! cargo test --release -p calimero-context --lib insert_capacity -- --ignored --nocapture
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_primitives::context::ContextId;
use serde_json::{json, Value};
use tracing::field::{Field, Visit};
use tracing::span;

use super::Chat;
use crate::search::NodeContextSource;

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[(((sorted.len() - 1) as f64) * p).round() as usize]
}

fn fmt(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us >= 1000.0 {
        format!("{:.2} ms", us / 1000.0)
    } else {
        format!("{us:.0} µs")
    }
}

/// `(p50, p95, p99)` of `n` sequential runs of `f`.
async fn timed<F, Fut>(n: usize, mut f: F) -> (Duration, Duration, Duration)
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut times = Vec::with_capacity(n);
    for i in 0..n {
        let t = Instant::now();
        f(i).await;
        times.push(t.elapsed());
    }
    times.sort();
    (
        percentile(&times, 0.5),
        percentile(&times, 0.95),
        percentile(&times, 0.99),
    )
}

/// The gas of the last execution. The runtime reports it only in a `debug!`
/// event (`gas_used`, target `calimero_runtime`), so the benchmark installs
/// [`GasTap`] as the global subscriber and reads it back from there.
static LAST_GAS: AtomicU64 = AtomicU64::new(0);

/// A subscriber that keeps only the runtime's `gas_used` event.
struct GasTap;

impl Visit for GasTap {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "gas_used" {
            LAST_GAS.store(value, Ordering::Relaxed);
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

impl tracing::Subscriber for GasTap {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.is_event()
            && metadata.target().starts_with("calimero_runtime")
            && metadata.fields().field("gas_used").is_some()
    }

    fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }

    fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

    fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        event.record(&mut GasTap);
    }

    fn enter(&self, _span: &span::Id) {}

    fn exit(&self, _span: &span::Id) {}
}

/// Run `method` once: its result (or why it failed, e.g. exhausted gas) and
/// the gas it used.
async fn metered(
    chat: &Chat,
    context: ContextId,
    method: &str,
    args: &Value,
) -> (Result<Value, String>, u64) {
    LAST_GAS.store(0, Ordering::Relaxed);
    let response = chat
        .harness
        .context_client
        .execute(
            &context,
            &chat.executor,
            method.to_owned(),
            serde_json::to_vec(args).expect("json"),
            None,
        )
        .await;
    let gas = LAST_GAS.load(Ordering::Relaxed);
    let result = match response {
        Ok(response) => match response.returns {
            Ok(Some(bytes)) => Ok(serde_json::from_slice(&bytes).expect("json result")),
            Ok(None) => Ok(Value::Null),
            Err(err) => Err(format!("{err:?}")),
        },
        Err(err) => Err(format!("{err:?}")),
    };
    (result, gas)
}

fn gas(g: u64) -> String {
    format!("{:.2} M", g as f64 / 1e6)
}

/// The process's peak resident set, from `/proc/self/status` (Linux only).
fn peak_rss() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|l| l.starts_with("VmHWM:"))
                .map(|l| l.trim_start_matches("VmHWM:").trim().to_owned())
        })
        .unwrap_or_else(|| "unavailable".to_owned())
}

fn corpus_text(i: usize) -> String {
    const WORDS: [&str; 16] = [
        "ka", "ri", "to", "me", "su", "lo", "na", "pe", "vi", "do", "ga", "hu", "ze", "bo", "fi",
        "ly",
    ];
    let mut x = (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut words = Vec::new();
    for _ in 0..10 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        // Zipf-ish: the square of a uniform draw skews toward low ranks.
        let u = (x % 10_000) as f64 / 10_000.0;
        let rank = ((u * u) * 4096.0) as usize;
        words.push(format!(
            "{}{}{}",
            WORDS[rank % 16],
            WORDS[(rank / 16) % 16],
            WORDS[(rank / 256) % 16]
        ));
    }
    if i % 200 == 7 {
        words.push("needle".to_owned());
    }
    if i % 2_000 == 3 {
        words.push("zebrafish".to_owned());
    }
    words.join(" ")
}

/// The end-to-end numbers for `tools/search-bench/README.md`.
#[actix::test]
#[ignore = "benchmark; run in release"]
async fn search_e2e_bench() {
    let n: usize = std::env::var("SEARCH_BENCH_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    // Taken before any execution, so every runtime callsite registers with it.
    let _ = tracing::subscriber::set_global_default(GasTap);
    let on = Chat::new(1, true).await;
    let off = Chat::new(1, false).await;
    let (a, z) = (on.contexts[0], off.contexts[0]);
    println!("\n## End-to-end (live ContextManager, search-chat wasm, RocksDB), {n} messages\n");

    // Build the (empty) index first, so the seeding below reaches it through
    // the dirty log, the way live writes do.
    let _ = on.index(a).await;

    // Seed through the app, 500 messages per execution.
    let t = Instant::now();
    for chunk in (0..n).collect::<Vec<_>>().chunks(500) {
        let batch: Vec<Value> = chunk
            .iter()
            .map(|i| json!({ "id": format!("m{i:07}"), "sender": format!("user{}", i % 50), "text": corpus_text(*i), "ts": *i as u64 }))
            .collect();
        let _ = on
            .call(a, "post_many", json!({ "messages": batch }))
            .await
            .expect("seed");
        let _ = off
            .call(z, "post_many", json!({ "messages": batch }))
            .await
            .expect("seed");
    }
    println!(
        "Seeded both contexts ({n} messages each, 500 per execution) in {:.1} s.\n",
        t.elapsed().as_secs_f64()
    );
    let rows = on.dirty_rows(a);
    let row_bytes: usize = rows.iter().map(|r| row_size(r.ids.len())).sum();
    let ids: usize = rows.iter().map(|r| r.ids.len()).sum();
    println!(
        "Dirty log after seeding: {} rows (one per execution; {:.1} ids, {} B per row on average).\n",
        rows.len(),
        ids as f64 / rows.len() as f64,
        row_bytes / rows.len()
    );

    // Index the backlog: extraction through the app's views + tantivy.
    let t = Instant::now();
    let report = on.index(a).await;
    let total = t.elapsed();
    println!(
        "Incremental, bulk: draining the {n}-message dirty backlog took {:.2} s = {} per message (extract through the wasm view {}, tantivy {}); {} rows, {} ids, {} docs, {} commits.\n",
        total.as_secs_f64(),
        fmt(total / n as u32),
        fmt(report.extract_time / n as u32),
        fmt(report.index_time / n as u32),
        report.rows,
        report.ids,
        report.docs,
        report.commits
    );
    // Full rebuild (first build / snapshot / version bump): drop it, scan it back.
    let search = Arc::clone(on.service());
    search.delete_context(a.as_ref()).expect("drop the index");
    let t = Instant::now();
    let report = on.index(a).await;
    let total = t.elapsed();
    println!(
        "Full build from a scan of state: {:.2} s = {} per message (scan through the wasm view {}, tantivy {}); {} documents.\n",
        total.as_secs_f64(),
        fmt(total / n as u32),
        fmt(report.extract_time / n as u32),
        fmt(report.index_time / n as u32),
        report.rebuilt
    );

    // Write-path overhead: single-message posts, search on vs off.
    let iters = 200;
    let post = |i: usize| json!({ "id": format!("x{i}"), "sender": "u", "text": corpus_text(i + n), "ts": 1 });
    let (on50, on95, on99) = timed(iters, |i| {
        let (on, args) = (&on, post(i));
        async move {
            let _ = on.call(a, "post", args).await.expect("post");
        }
    })
    .await;
    let (off50, off95, off99) = timed(iters, |i| {
        let (off, args) = (&off, post(i));
        async move {
            let _ = off.call(z, "post", args).await.expect("post");
        }
    })
    .await;
    let (_, on_gas) = metered(&on, a, "post", &post(iters)).await;
    let (_, off_gas) = metered(&off, z, "post", &post(iters)).await;
    let single = on.dirty_rows(a);
    let last = single.last().expect("a row");
    println!(
        "| one `post` execution at {n} messages | p50 | p95 | p99 | gas |\n|---|---|---|---|---|"
    );
    println!(
        "| search off | {} | {} | {} | {} |",
        fmt(off50),
        fmt(off95),
        fmt(off99),
        gas(off_gas)
    );
    println!(
        "| search on (dirty row staged in the batch) | {} | {} | {} | {} |",
        fmt(on50),
        fmt(on95),
        fmt(on99),
        gas(on_gas)
    );
    println!(
        "\nA single post's dirty row names {} entity ids = {} B (key 40 + value {}), plus the 40 B counter update.\n",
        last.ids.len(),
        row_size(last.ids.len()),
        row_size(last.ids.len()) - 40
    );
    let posts = single.len();
    let t = Instant::now();
    let report = on.index(a).await;
    let total = t.elapsed();
    println!(
        "Incremental, per send: indexing the {posts} single-post rows took {} = {} per post (extract {}, tantivy {}, {} commits).\n",
        fmt(total),
        fmt(total / posts as u32),
        fmt(report.extract_time / posts as u32),
        fmt(report.index_time / posts as u32),
        report.commits
    );

    // Queries through the view (wasm + host fn + re-read of each hit).
    println!(
        "| query through the `search` view | total | p50 | p95 | p99 | gas |\n|---|---|---|---|---|---|"
    );
    let (p50, p95, p99) = timed(50, |_| {
        let on = &on;
        async move {
            let _ = on.call(a, "count", json!({})).await.expect("count");
        }
    })
    .await;
    let (_, count_gas) = metered(&on, a, "count", &json!({})).await;
    println!(
        "| (floor: the `count` view, no search) | — | {} | {} | {} | {} |",
        fmt(p50),
        fmt(p95),
        fmt(p99),
        gas(count_gas)
    );
    for (label, query, mode, sender) in [
        ("rare word", "zebrafish", "words", None),
        ("common word, top-20", "kakaka", "words", None),
        ("no match", "qqxqq", "words", None),
        ("prefix", "needl", "prefix", None),
        ("infix substring", "eedl", "substring", None),
        ("two-term AND", "needle kakaka", "words", None),
        ("sender facet + word", "kakaka", "words", Some("user3")),
        ("fuzzy (distance 1)", "neadle", "fuzzy", None),
    ] {
        let args = json!({ "query": query, "mode": mode, "sender": sender });
        let (result, used) = metered(&on, a, "search", &args).await;
        let total = result.expect("search")["total"].clone();
        let (p50, p95, p99) = timed(50, |_| {
            let (on, args) = (&on, args.clone());
            async move {
                let _ = on.call(a, "search", args).await.expect("search");
            }
        })
        .await;
        println!(
            "| {label} (`{query}`) | {total} | {} | {} | {} | {} |",
            fmt(p50),
            fmt(p95),
            fmt(p99),
            gas(used)
        );
    }
    println!(
        "\n| baseline: `scan_search` view (lowercase substring over every message) | total | p50 | p95 | gas |\n|---|---|---|---|---|"
    );
    for (label, term) in [
        ("rare word", "zebrafish"),
        ("common word", "kakaka"),
        ("no match", "qqxqq"),
    ] {
        let args = json!({ "term": term });
        let (result, used) = metered(&on, a, "scan_search", &args).await;
        let (p50, p95, _) = timed(10, |_| {
            let (on, args) = (&on, args.clone());
            async move {
                let _ = metered(on, a, "scan_search", &args).await;
            }
        })
        .await;
        let total = match result {
            Ok(page) => page["total"].to_string(),
            Err(err) if err.contains("exhausted its gas") => "gas exhausted".to_owned(),
            Err(err) => panic!("scan_search failed: {err}"),
        };
        println!(
            "| {label} (`{term}`) | {total} | {} | {} | {} |",
            fmt(p50),
            fmt(p95),
            gas(used)
        );
    }

    // Freshness through the real indexer loop.
    let indexer = tokio::spawn(search.run_indexer(Arc::new(NodeContextSource::new(
        on.harness.context_client.clone(),
    ))));
    let mut lags = Vec::new();
    for i in 0..20 {
        let token = format!("fresh{i}token");
        let _ = on.call(a, "post", json!({ "id": format!("f{i}"), "sender": "u", "text": format!("hello {token}"), "ts": 1 })).await.expect("post");
        let t = Instant::now();
        loop {
            if on.search(a, &token, "words").await["total"] == 1 {
                break;
            }
            assert!(
                t.elapsed() < Duration::from_secs(10),
                "{token} never indexed"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        lags.push(t.elapsed());
        tokio::time::sleep(Duration::from_millis((i * 37 % 250) as u64)).await;
    }
    indexer.abort();
    lags.sort();
    println!(
        "\nFreshness through the live indexer (250 ms commit interval, measured from the post returning): p50 {}, p95 {}, max {}.\n",
        fmt(percentile(&lags, 0.5)),
        fmt(percentile(&lags, 0.95)),
        fmt(*lags.last().expect("lags"))
    );
    println!("Peak RSS of the whole benchmark process (both contexts, wasm engine, RocksDB, index): {}.\n", peak_rss());
}

/// Bytes of a dirty row naming `ids` entity ids: the 40-byte key, then the
/// borsh record (a version tag, two roots, the id vector).
fn row_size(ids: usize) -> usize {
    40 + 1 + 64 + 4 + 32 * ids
}

/// `post_many` of `n` fresh messages on `context`: its gas, and why it
/// failed if it did.
async fn post_many(
    chat: &Chat,
    context: ContextId,
    from: usize,
    n: usize,
) -> (u64, Option<String>) {
    let batch: Vec<Value> = (from..from + n)
        .map(|i| json!({ "id": format!("m{i:07}"), "sender": format!("user{}", i % 50), "text": corpus_text(i), "ts": i as u64 }))
        .collect();
    let (result, gas) = metered(chat, context, "post_many", &json!({ "messages": batch })).await;
    (gas, result.err())
}

/// Part of the capacity question: how many inserts fit in one call under the
/// default 1e9 gas budget, what one costs, and whether that grows with the
/// collection. Run with search off and on; the gas must agree, because the
/// dirty row is host bookkeeping after the run and a write can never search.
#[actix::test]
#[ignore = "benchmark; run in release"]
async fn insert_capacity() {
    let _ = tracing::subscriber::set_global_default(GasTap);
    let sizes: Vec<usize> = std::env::var("SEARCH_BENCH_SIZES")
        .ok()
        .map(|s| s.split(',').filter_map(|n| n.parse().ok()).collect())
        .unwrap_or_else(|| vec![2_000, 10_000, 50_000, 200_000]);
    println!(
        "\n## Inserts under the default gas budget ({} gas)\n",
        gas(calimero_runtime::logic::VMLimits::default().max_gas)
    );
    println!("| search | gas of 1 / 10 / 100 / 200 inserts in one call | fixed per call | per insert (100 → 200) | most inserts in one call | what stops the next one |\n|---|---|---|---|---|---|");
    let mut batch = usize::MAX;
    for search_on in [false, true] {
        // A fresh context per probe, so every batch lands on an empty map.
        let chat = Chat::new(40, search_on).await;
        let mut contexts = chat.contexts.clone().into_iter();
        let mut probe = |n: usize| {
            let context = contexts.next().expect("enough contexts for the probes");
            let chat = &chat;
            async move { post_many(chat, context, 0, n).await }
        };
        let mut g = Vec::new();
        for n in [1, 10, 100, 200] {
            let (used, err) = probe(n).await;
            assert!(err.is_none(), "{n} inserts failed: {err:?}");
            g.push(used);
        }
        let per = (g[3] - g[2]) as f64 / 100.0;
        let fixed = g[0] as f64 - per;
        // Double until a batch fails, then bisect.
        let (mut lo, mut hi) = (200_usize, 400_usize);
        let mut stop;
        loop {
            match probe(hi).await {
                (_, None) => {
                    lo = hi;
                    hi *= 2;
                }
                (_, Some(err)) => {
                    stop = err;
                    break;
                }
            }
        }
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            match probe(mid).await {
                (_, None) => lo = mid,
                (_, Some(err)) => {
                    hi = mid;
                    stop = err;
                }
            }
        }
        let (at_max, _) = probe(lo).await;
        batch = batch.min(lo);
        let reason = if stop.contains("exhausted its gas") {
            "gas exhausted".to_owned()
        } else {
            stop.chars().take(120).collect()
        };
        println!(
            "| {} | {} / {} / {} / {} | {} | {} | {lo} ({} gas) | {reason} |",
            if search_on { "on" } else { "off" },
            gas(g[0]),
            gas(g[1]),
            gas(g[2]),
            gas(g[3]),
            gas(fixed as u64),
            gas(per as u64),
            gas(at_max),
        );
    }

    println!("\n| messages already in the map | one `post`, search off | one `post`, search on |\n|---|---|---|");
    let off = Chat::new(1, false).await;
    let on = Chat::new(1, true).await;
    let (z, a) = (off.contexts[0], on.contexts[0]);
    let mut seeded = 0;
    for size in sizes {
        while seeded < size {
            let n = (size - seeded).min(batch * 4 / 5);
            for (chat, context) in [(&off, z), (&on, a)] {
                let (_, err) = post_many(chat, context, seeded, n).await;
                assert!(err.is_none(), "seeding failed: {err:?}");
            }
            seeded += n;
        }
        // Drain the backlog as the live indexer would, so the dirty log is
        // not what grows.
        let _ = on.index(a).await;
        let args =
            json!({ "id": format!("p{size}"), "sender": "u", "text": corpus_text(size), "ts": 1 });
        let (_, off_gas) = metered(&off, z, "post", &args).await;
        let (_, on_gas) = metered(&on, a, "post", &args).await;
        println!("| {size} | {} | {} |", gas(off_gas), gas(on_gas));
    }
}
