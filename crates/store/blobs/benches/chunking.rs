//! What does storing a blob cost per byte?

use std::hint::black_box;
use std::sync::Arc;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager, FileSystem};
use calimero_store::db::InMemoryDB;
use calimero_store::Store as DataStore;
use camino::Utf8PathBuf;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tempfile::TempDir;

// The `TempDir` must outlive the manager: dropping it deletes the blob store.
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
                // A fresh store per iteration: a repeat put would measure dedup.
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
