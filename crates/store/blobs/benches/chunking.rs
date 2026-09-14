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
//! `:411-412` for the two updates). Two software SHA-256 passes at roughly
//! 3-4 ns/byte each account for most of the ~7.9 ns/byte this bench measures;
//! the filesystem write is the smaller remainder, not the dominant cost.
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

criterion_group!(benches, chunking);
criterion_main!(benches);
