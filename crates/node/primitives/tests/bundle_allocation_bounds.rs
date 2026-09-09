//! Bundle reads whose bound is a resource bound, provable only by measuring:
//! a bomb `tar` would buffer inside `entries()` before any `Entry` reaches
//! caller code, and an archive walked once per artifact instead of once.
//!
//! Both the counting allocator and the clock are process-wide, so every
//! measurement here takes `PROBE`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use calimero_node_primitives::bundle::{derive_signer_id_did_key, sign_manifest_json};
use calimero_node_primitives::client::NodeClient;
use ed25519_dalek::SigningKey;
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::io::Cursor;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use calimero_node_primitives::test_fixtures::{hex_lower, node_client, pack_entries};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn record(live: usize) {
    let _ = PEAK.fetch_max(live, Ordering::Relaxed);
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record(LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record(LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        let _ = LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            if new_size >= layout.size() {
                let grew = new_size - layout.size();
                record(LIVE.fetch_add(grew, Ordering::Relaxed) + grew);
            } else {
                let _ = LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Serializes measurement, since the counter is process-wide and the harness
/// runs tests in parallel threads.
static PROBE: Mutex<()> = Mutex::new(());

/// Peak bytes allocated while `body` runs, excluding whatever was already live.
fn peak_while<T>(body: impl FnOnce() -> T) -> (T, usize) {
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let out = body();
    (out, PEAK.load(Ordering::Relaxed).saturating_sub(before))
}

/// Bytes the bomb expands to. Comfortably over the manifest scan cap, and small
/// enough that the archive builds in well under a second.
const BOMB_BYTES: usize = 64 * 1024 * 1024;

/// A `.mpk` whose first member is a GNU long-name header declaring, and
/// carrying, `BOMB_BYTES` of zeros. Roughly 64 KiB on the wire.
fn long_name_bomb() -> Vec<u8> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::GNULongName);
    header.set_size(BOMB_BYTES as u64);
    header.set_cksum();

    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(header.as_bytes()).unwrap();
    let chunk = vec![0u8; 64 * 1024];
    for _ in 0..(BOMB_BYTES / chunk.len()) {
        encoder.write_all(&chunk).unwrap();
    }
    encoder.finish().unwrap()
}

#[test]
fn long_name_bomb_is_refused_without_buffering_it() {
    let _probe = PROBE.lock().unwrap_or_else(|e| e.into_inner());
    // Built before the baseline is taken, so the fixture's own bytes are excluded.
    let archive = long_name_bomb();

    let (verdict, peak) = peak_while(|| NodeClient::is_bundle_blob(&archive));

    assert!(
        !verdict,
        "a bomb carrying no manifest.json must not read as a bundle"
    );
    // Unbounded, `tar` buffers the declared size and Vec doubling lands near 3x
    // it. Anything at or above the payload itself means the bound did not hold.
    assert!(
        peak < BOMB_BYTES / 2,
        "peak allocation {peak} bytes for a {} byte archive declaring {BOMB_BYTES} bytes: \
         the bomb was buffered",
        archive.len()
    );
}

/// Bytes of the one artifact every service names. Big enough to dwarf harness
/// noise, small enough to stay quick.
const SHARED_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
const SERVICES: usize = 8;

/// A signed `.mpk` whose `SERVICES` services all name one `app.wasm`.
fn many_services_one_artifact(dir: &TempDir, wasm: &[u8]) -> Vec<u8> {
    let key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let services: Vec<_> = (0..SERVICES)
        .map(|i| {
            serde_json::json!({
                "name": format!("svc{i}"),
                "wasm": {
                    "path": "app.wasm",
                    "size": wasm.len(),
                    "hash": hex_lower(&Sha256::digest(wasm)),
                },
            })
        })
        .collect();
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "com.example.manyservices",
        "appVersion": "1.0.0",
        "signerId": derive_signer_id_did_key(key.verifying_key().as_bytes()),
        "minRuntimeVersion": "0.1.0",
        "services": services,
        "migrations": []
    });
    sign_manifest_json(&mut manifest, &key).unwrap();
    pack_entries(
        dir,
        "manyservices.mpk",
        &[
            ("manifest.json", &serde_json::to_vec(&manifest).unwrap()),
            ("app.wasm", wasm),
        ],
    )
}

/// Install holds every service's bytes at once, so reading one path once is the
/// only thing between a manifest naming it N times and N copies in memory.
#[test]
fn services_sharing_one_artifact_cost_one_copy() {
    let _probe = PROBE.lock().unwrap_or_else(|e| e.into_inner());

    let dir = TempDir::new().unwrap();
    let wasm = vec![0xABu8; SHARED_ARTIFACT_BYTES];
    let bundle = many_services_one_artifact(&dir, &wasm);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (node, _store, _data, _blobs) = runtime.block_on(node_client());
    let (blob_id, _) = runtime
        .block_on(node.add_blob(
            Cursor::new(bundle.as_slice()),
            Some(bundle.len() as u64),
            None,
        ))
        .unwrap();
    let source = "https://registry.example.com/app.mpk".parse().unwrap();

    let (installed, peak) = peak_while(|| {
        runtime.block_on(node.install_application_from_bundle_blob(&blob_id, &source))
    });
    installed.expect("multi-service install");

    // One copy costs ~3x the artifact, not 1x: the read buffer and the `Arc` it
    // is copied into are both live. N copies cost ~10x, so half of N separates them.
    assert!(
        peak < SHARED_ARTIFACT_BYTES * SERVICES / 2,
        "peak allocation {peak} bytes installing {SERVICES} services that name one \
         {SHARED_ARTIFACT_BYTES} byte artifact: the shared read is not shared"
    );
}

/// Zero-filled padding entry: ~64 MiB to inflate, ~64 KiB on the wire. Skipping
/// an entry still decompresses it, so this is what a redundant walk pays for.
const PADDING_BYTES: usize = 64 * 1024 * 1024;

/// A signed `.mpk` with `services` distinct tiny wasm artifacts, each named by
/// one service, optionally behind a `padding` byte entry the walk must cross.
fn padded_bundle(dir: &TempDir, name: &str, services: usize, padding: usize) -> Vec<u8> {
    let key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let wasms: Vec<Vec<u8>> = (0..services)
        .map(|i| format!("wasm for service {i}").into_bytes())
        .collect();
    let declared: Vec<_> = wasms
        .iter()
        .enumerate()
        .map(|(i, wasm)| {
            serde_json::json!({
                "name": format!("svc{i}"),
                "wasm": {
                    "path": format!("svc{i}.wasm"),
                    "size": wasm.len(),
                    "hash": hex_lower(&Sha256::digest(wasm)),
                },
            })
        })
        .collect();
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": format!("com.example.padded{services}"),
        "appVersion": "1.0.0",
        "signerId": derive_signer_id_did_key(key.verifying_key().as_bytes()),
        "minRuntimeVersion": "0.1.0",
        "services": declared,
        "migrations": []
    });
    sign_manifest_json(&mut manifest, &key).unwrap();

    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let padding = vec![0u8; padding];
    let paths: Vec<String> = (0..services).map(|i| format!("svc{i}.wasm")).collect();
    let mut entries: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest_bytes)];
    if !padding.is_empty() {
        // Ahead of the artifacts, so every read has to walk through it.
        entries.push(("padding.bin", &padding));
    }
    entries.extend(
        paths
            .iter()
            .map(|p| p.as_str())
            .zip(wasms.iter().map(Vec::as_slice)),
    );
    pack_entries(dir, name, &entries)
}

fn install_elapsed(bundle: Vec<u8>) -> Duration {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (node, _store, _data, _blobs) = runtime.block_on(node_client());
    let (blob_id, _) = runtime
        .block_on(node.add_blob(
            Cursor::new(bundle.as_slice()),
            Some(bundle.len() as u64),
            None,
        ))
        .unwrap();
    let source = "https://registry.example.com/app.mpk".parse().unwrap();

    let started = Instant::now();
    runtime
        .block_on(node.install_application_from_bundle_blob(&blob_id, &source))
        .expect("install");
    started.elapsed()
}

/// What crossing the padding costs during one install. Everything else about
/// the two installs is identical, so the difference isolates the walk from a
/// fixed cost that would otherwise swamp it.
fn walk_cost(dir: &TempDir, services: usize) -> Duration {
    let padded = padded_bundle(dir, &format!("pad{services}.mpk"), services, PADDING_BYTES);
    let bare = padded_bundle(dir, &format!("bare{services}.mpk"), services, 0);
    install_elapsed(padded).saturating_sub(install_elapsed(bare))
}

/// Reading D distinct artifacts must cost one walk, not D. Nothing caps
/// `services[]`, so a walk per artifact multiplies the archive's whole
/// decompressed size by D, and D can reach thousands in a small `.mpk`.
#[test]
fn distinct_artifacts_cost_one_walk() {
    let _probe = PROBE.lock().unwrap_or_else(|e| e.into_inner());

    const DISTINCT: usize = 8;
    let dir = TempDir::new().unwrap();
    let one = walk_cost(&dir, 1);
    let many = walk_cost(&dir, DISTINCT);

    // One walk either way, so the padding is crossed once in both. A walk per
    // artifact would make it ~{DISTINCT}x, since each read re-inflates it.
    assert!(
        many < one * 3,
        "crossing {PADDING_BYTES} bytes of padding cost {many:?} for {DISTINCT} distinct \
         artifacts against {one:?} for one: the archive is being walked per artifact"
    );
}
