//! What does storing a blob cost per byte?
//!
//! `BlobManager::put` splits the stream into 1 MiB chunks (`CHUNK_SIZE`,
//! `src/lib.rs:31`), SHA-256s each chunk to get its id, writes it, and then
//! hashes the chunk ids together to get the root id. Every image, avatar and
//! application bundle pays this on the way in, and the receiving node pays it
//! again on the way out.
//!
//! Sizes bracket the chunk boundary on purpose: just under one chunk
//! (1,000,000 bytes), exactly one full chunk (1,048,576 bytes — `CHUNK_SIZE`
//! is `1 << 20` exactly, `src/lib.rs:31`), and several (4,194,304 bytes, four
//! chunks). A per-byte cost that jumps between the first two sizes means the
//! chunking path, not the hashing, dominates; the observed measurement is
//! flat across that boundary (see the benchmark report).
//!
//! Measured throughput lands around ~120-126 MiB/s, well short of the naive
//! "hundreds of MB/s" a single SHA-256 pass would suggest, because
//! `put_sized` hashes every chunk **twice** into two independent `Sha256`
//! instances over the same bytes: `blob.digest.update(chunk)` accumulates the
//! root id and `file.digest.update(chunk)` accumulates that chunk's own id
//! (`src/lib.rs:132-135` for the `State` struct holding both digests,
//! `:411-412` for the two updates).
//!
//! # Splitting hashing from the filesystem (2026-08-31 measurement)
//!
//! The paragraph above was arithmetic ("two SHA-256 passes at roughly 3-4
//! ns/byte each"), not a measurement of this codebase's actual SHA-256 cost
//! against actual `put` numbers side by side. The `hash_sha256_x2` group
//! below measures it directly: two back-to-back `Sha256::update` passes over
//! the SAME in-memory buffer (mirroring `put_sized`'s two-digest shape
//! exactly, with no filesystem or `BlobManager` in the loop at all) at the
//! same three sizes as `put`. Compare its ns/byte against `put`'s: the
//! difference is the filesystem-write-plus-dispatch share, not attributed by
//! arithmetic. See the report this bench feeds
//! (`.superpowers/sdd/2026-08-31-core-benchmark-suite/final-measurements.md`)
//! for the resulting split and whether it confirms "most of it is hashing".
//!
//! Context for the numbers: a receiver abandons a transfer after 60s
//! (`crates/network/.../request_blob.rs:18`), so the p2p ceiling of 500 MiB
//! implies a sustained 8.5 MB/s. If local chunking alone cannot beat that, the
//! transfer limit is unreachable for reasons that have nothing to do with the
//! network.

use std::hint::black_box;
use std::path::Path;
use std::sync::Arc;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager, FileSystem};
use calimero_store::db::InMemoryDB;
use calimero_store::Store as DataStore;
use camino::Utf8PathBuf;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// Mirrors the crate's own test setup (`src/lib.rs:890-900`): an in-memory
/// data store for metadata plus a real `FileSystem` blob store rooted in a
/// fresh temp directory. `FileSystem::new` is async, so this is called from
/// setup on a throwaway runtime, never from inside a timed closure. The
/// `TempDir` is returned alongside the manager so it outlives the
/// measurement — dropping it would delete the store out from under `put`.
async fn new_manager_async(root: &Path) -> BlobManager {
    let data_store = DataStore::new(Arc::new(InMemoryDB::owned()));
    let config = BlobStoreConfig::new(Utf8PathBuf::from_path_buf(root.to_path_buf()).unwrap());
    let blob_store = FileSystem::new(&config).await.unwrap();
    BlobManager::new(data_store, blob_store)
}

fn new_manager(runtime: &tokio::runtime::Runtime) -> (BlobManager, TempDir) {
    let dir = tempfile::tempdir().expect("a temp dir must be creatable");
    let manager = runtime.block_on(new_manager_async(dir.path()));
    (manager, dir)
}

fn chunking(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime must build");

    let mut group = c.benchmark_group("blob");

    // Just under one chunk, exactly one, and four.
    for bytes in [1_000_000_usize, 1_048_576, 4_194_304] {
        let payload = vec![0xCD_u8; bytes];

        group.throughput(Throughput::Bytes(bytes as u64));
        group.bench_with_input(BenchmarkId::new("put", bytes), &payload, |b, payload| {
            b.iter_batched(
                // A fresh store per iteration: content-addressing means the
                // second put of identical bytes is a no-op, which would
                // measure deduplication rather than chunking.
                || new_manager(&runtime),
                |(manager, _tempdir)| {
                    runtime.block_on(async {
                        black_box(manager.put(&payload[..]).await.expect("put must succeed"))
                    })
                },
                criterion::BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

/// Isolates the two-SHA-256-passes cost `put_sized` pays per chunk
/// (`src/lib.rs:132-135`, `:411-412`) from everything else `put` also pays
/// (chunk-boundary bookkeeping, the `BlobManager`/`FileSystem` dispatch, and
/// the actual filesystem write) — same sizes as the `put` group above, no
/// filesystem or async runtime in the timed section at all. `Digest::update`
/// processes data in fixed internal blocks regardless of how many calls
/// supply it, so hashing the payload as one `update` per digest measures the
/// identical total SHA-256 cost `put_sized`'s per-1MiB-chunk `update` calls
/// pay — only the call-count differs, not the bytes actually run through the
/// compression function.
fn hash_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_sha256_x2");

    for bytes in [1_000_000_usize, 1_048_576, 4_194_304] {
        let payload = vec![0xCD_u8; bytes];

        group.throughput(Throughput::Bytes(bytes as u64));
        group.bench_with_input(
            BenchmarkId::new("hash_sha256_x2", bytes),
            &payload,
            |b, payload| {
                b.iter(|| {
                    // Mirrors `put_sized`'s `State`: one digest accumulating
                    // the root id, one accumulating this chunk's own id, both
                    // fed the SAME bytes.
                    let mut root_digest = Sha256::new();
                    let mut chunk_digest = Sha256::new();
                    root_digest.update(black_box(&payload[..]));
                    chunk_digest.update(black_box(&payload[..]));
                    black_box((root_digest.finalize(), chunk_digest.finalize()))
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, chunking, hash_only);
criterion_main!(benches);
