//! What does storing a blob cost per byte?
//!
//! `BlobManager::put` splits the stream into `CHUNK_SIZE` (1 MiB) chunks,
//! SHA-256s each to get its id, writes it, then hashes the chunk ids together
//! to get the root id. Every image, avatar and application bundle pays this on
//! the way in, and the receiving node pays it again on the way out.
//!
//! The sizes bracket the chunk boundary on purpose: just under one chunk,
//! exactly one, and four. A per-byte cost that jumps between the first two
//! means the chunking path rather than the hashing dominates; it is currently
//! flat across that boundary.
//!
//! Throughput lands around 120-126 MiB/s rather than the "hundreds of MB/s" a
//! single SHA-256 pass would suggest, because `put_sized` hashes every chunk
//! TWICE over the same bytes, once into the root id and once into that chunk's
//! own id. The filesystem write is the smaller remainder.
//!
//! For scale: a receiver abandons a transfer after 60s, so the 500 MiB p2p
//! ceiling implies a sustained 8.5 MB/s. If local chunking alone cannot beat
//! that, the transfer limit is unreachable for reasons that have nothing to do
//! with the network.

use std::hint::black_box;
use std::sync::Arc;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager, FileSystem};
use calimero_store::db::InMemoryDB;
use calimero_store::Store as DataStore;
use camino::Utf8PathBuf;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tempfile::TempDir;

/// Mirrors the crate's own test setup: in-memory metadata plus a real
/// `FileSystem` blob store in a fresh temp directory. The `TempDir` is
/// returned alongside the manager so it outlives the measurement; dropping it
/// would delete the store out from under `put`.
fn new_manager(runtime: &tokio::runtime::Runtime) -> (BlobManager, TempDir) {
    let dir = tempfile::tempdir().expect("a temp dir must be creatable");
    let manager = runtime.block_on(async {
        let data_store = DataStore::new(Arc::new(InMemoryDB::owned()));
        let config =
            BlobStoreConfig::new(Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap());
        let blob_store = FileSystem::new(&config).await.unwrap();
        BlobManager::new(data_store, blob_store)
    });
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
