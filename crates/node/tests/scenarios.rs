//! Every scenario file in the repository, each on its own in-process cluster
//! (`harness/scenario.rs` runs them; the formats are documented in
//! `docs/src/content/docs/contribute/testing.mdx`):
//!
//! - app-level scenarios, `apps/<app>/scenarios/<name>.json` (+ `<name>.org.json`);
//! - cluster scenarios, `apps/<app>/scenarios/cluster/**/*.json` and
//!   `workflows/**/scenarios/**/*.json`.
//!
//! A custom test main (`harness = false`): `cargo test -p calimero-node --test scenarios`
//! runs them all, `--test-threads N` at a time (default 2). Selection:
//! `SCENARIO_FILTER=a,b` (or filter arguments) keeps the paths containing any of them,
//! `SCENARIO_SHARD=k/n` the k-th of n shards (1-based, every n-th path in sorted order),
//! `--list` prints them. Scenarios listed in `tests/scenarios-skip.txt` (`<path> <reason>`)
//! are reported ignored unless `--include-ignored` / `--ignored` (or `SCENARIO_INCLUDE_SKIPPED=1`).

#[path = "harness/cluster.rs"]
#[allow(dead_code)]
mod cluster;
#[path = "harness/py.rs"]
mod py;
#[path = "harness/scenario.rs"]
mod scenario;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    App,
    Cluster,
}

fn json_files(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if recursive {
                json_files(&p, true, out);
            }
        } else if p.extension().is_some_and(|x| x == "json")
            && !p.to_string_lossy().ends_with(".org.json")
        {
            out.push(p);
        }
    }
}

/// Every scenario file, as `(path relative to the repository root, kind)`, sorted.
fn discover(root: &Path) -> Vec<(String, Kind)> {
    let mut found = Vec::new();
    if let Ok(apps) = std::fs::read_dir(root.join("apps")) {
        for app in apps.flatten() {
            let dir = app.path().join("scenarios");
            let mut flat = Vec::new();
            json_files(&dir, false, &mut flat);
            found.extend(flat.into_iter().map(|p| (p, Kind::App)));
            let mut nested = Vec::new();
            json_files(&dir.join("cluster"), true, &mut nested);
            found.extend(nested.into_iter().map(|p| (p, Kind::Cluster)));
        }
    }
    fn workflow_scenarios(dir: &Path, out: &mut Vec<(PathBuf, Kind)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            if p.file_name().is_some_and(|n| n == "scenarios") {
                let mut files = Vec::new();
                json_files(&p, true, &mut files);
                out.extend(files.into_iter().map(|f| (f, Kind::Cluster)));
            } else {
                workflow_scenarios(&p, out);
            }
        }
    }
    workflow_scenarios(&root.join("workflows"), &mut found);
    let mut out: Vec<(String, Kind)> = found
        .into_iter()
        .map(|(p, k)| {
            (
                p.strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .into_owned(),
                k,
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// `tests/scenarios-skip.txt`: `<path> <reason>` per line, `#` comments.
fn skip_list() -> BTreeMap<String, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scenarios-skip.txt");
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| match l.split_once(char::is_whitespace) {
            Some((p, why)) => (p.to_owned(), why.trim().to_owned()),
            None => (l.to_owned(), String::new()),
        })
        .collect()
}

struct Args {
    threads: usize,
    filters: Vec<String>,
    exact: bool,
    list: bool,
    include_skipped: bool,
}

fn args() -> Args {
    let mut a = Args {
        threads: std::env::var("RUST_TEST_THREADS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2),
        filters: std::env::var("SCENARIO_FILTER")
            .map(|f| {
                f.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        exact: false,
        list: false,
        include_skipped: std::env::var("SCENARIO_INCLUDE_SKIPPED").is_ok_and(|v| v == "1"),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--list" => a.list = true,
            "--exact" => a.exact = true,
            "--ignored" | "--include-ignored" => a.include_skipped = true,
            "--test-threads" => {
                if let Some(n) = it.next().and_then(|v| v.parse().ok()) {
                    a.threads = n;
                }
            }
            s if s.starts_with("--test-threads=") => {
                if let Ok(n) = s["--test-threads=".len()..].parse() {
                    a.threads = n;
                }
            }
            // libtest flags with a value this main has no use for.
            "--format" | "--color" | "--skip" | "-Z" | "--logfile" => {
                let _ = it.next();
            }
            s if s.starts_with('-') => {}
            s => a.filters.push(s.to_owned()),
        }
    }
    a.threads = a.threads.max(1);
    a
}

/// `SCENARIO_SHARD=k/n`.
fn shard() -> Option<(usize, usize)> {
    let v = std::env::var("SCENARIO_SHARD").ok()?;
    let (k, n) = v.split_once('/')?;
    let (k, n): (usize, usize) = (k.trim().parse().ok()?, n.trim().parse().ok()?);
    assert!(
        n >= 1 && (1..=n).contains(&k),
        "SCENARIO_SHARD={v}: want k/n, 1 <= k <= n"
    );
    Some((k, n))
}

fn run_one(root: &Path, path: &str, kind: Kind) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(6)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let full = root.join(path);
    rt.block_on(async move {
        match kind {
            Kind::App => scenario::run_app_scenario(&full).await,
            Kind::Cluster => scenario::run_cluster_scenario(&full).await,
        }
    })
}

fn main() {
    scenario::self_check();
    let a = args();
    let root = scenario::root();
    let skips = skip_list();
    let mut selected: Vec<(String, Kind)> = discover(&root)
        .into_iter()
        .filter(|(p, _)| {
            a.filters.is_empty()
                || a.filters.iter().any(|f| {
                    if a.exact {
                        p == f
                    } else {
                        p.contains(f.as_str())
                    }
                })
        })
        .collect();
    if let Some((k, n)) = shard() {
        selected = selected
            .into_iter()
            .enumerate()
            .filter(|(i, _)| i % n == k - 1)
            .map(|(_, x)| x)
            .collect();
    }
    if a.list {
        for (p, _) in &selected {
            println!("{p}: test");
        }
        return;
    }
    let (run, ignored): (Vec<_>, Vec<_>) = selected
        .into_iter()
        .partition(|(p, _)| a.include_skipped || !skips.contains_key(p));
    println!(
        "\nrunning {} scenarios ({} at a time)",
        run.len(),
        a.threads
    );
    for (p, _) in &ignored {
        println!("test {p} ... ignored, {}", skips[p]);
    }
    let queue = Arc::new(Mutex::new(
        run.into_iter().collect::<std::collections::VecDeque<_>>(),
    ));
    let failed: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let passed = Arc::new(Mutex::new(0usize));
    let started = Instant::now();
    let workers: Vec<_> = (0..a.threads)
        .map(|_| {
            let (queue, failed, passed, root) = (
                Arc::clone(&queue),
                Arc::clone(&failed),
                Arc::clone(&passed),
                root.clone(),
            );
            std::thread::spawn(move || loop {
                let Some((path, kind)) = queue.lock().unwrap().pop_front() else {
                    return;
                };
                let t = Instant::now();
                let (p2, r2) = (path.clone(), root.clone());
                let outcome = std::thread::Builder::new()
                    .name(path.clone())
                    .stack_size(16 << 20)
                    .spawn(move || run_one(&r2, &p2, kind))
                    .map_err(|e| e.to_string())
                    .and_then(|h| {
                        h.join().unwrap_or_else(|panic| {
                            let msg = panic
                                .downcast_ref::<String>()
                                .cloned()
                                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                                .unwrap_or_else(|| "panicked".into());
                            Err(format!("panicked: {msg}"))
                        })
                    });
                let secs = t.elapsed().as_secs_f64();
                let mut out = std::io::stdout().lock();
                match outcome {
                    Ok(()) => {
                        let _ = writeln!(out, "test {path} ... ok ({secs:.1}s)");
                        *passed.lock().unwrap() += 1;
                    }
                    Err(e) => {
                        let _ = writeln!(out, "test {path} ... FAILED ({secs:.1}s)");
                        failed.lock().unwrap().push((path, e));
                    }
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    let failed = failed.lock().unwrap().clone();
    let passed = *passed.lock().unwrap();
    if !failed.is_empty() {
        println!("\nfailures:\n");
        for (p, e) in &failed {
            println!("---- {p} ----\n{e}\n");
        }
        println!("failures:");
        for (p, _) in &failed {
            println!("    {p}");
        }
    }
    println!(
        "\ntest result: {}. {passed} passed; {} failed; {} ignored; finished in {:.1}s\n",
        if failed.is_empty() { "ok" } else { "FAILED" },
        failed.len(),
        ignored.len(),
        started.elapsed().as_secs_f64()
    );
    if !failed.is_empty() {
        std::process::exit(101);
    }
}
