//! What does encryption cost at the sizes this system actually moves?
//!
//! Every replicated byte goes through AES-256-GCM: sync deltas, gossip
//! envelopes (capped at 1 MiB by `GOSSIPSUB_MAX_TRANSMIT_SIZE`) and blob chunks
//! (exactly 1 MiB, `crates/store/blobs/src/lib.rs:31`). The sizes below are
//! those, not round numbers.
//!
//! `derive_shared_key` is measured separately because it is per-peer-pair, not
//! per-message: if it turned out to cost as much as encrypting a chunk, caching
//! it would matter. Today it should not.
//!
//! Both `encrypt_with_nonce` and `decrypt` take their payload by value and
//! encrypt/decrypt in place, so the `.clone()` inside each timed closure below
//! is unavoidable — it hands the API ownership of a fresh buffer every
//! iteration — and is deliberately part of what is measured, not setup noise.
//!
//! What would change a decision: an encrypt throughput materially below the
//! network's, which would make crypto — not bandwidth — the transfer ceiling.

use std::hint::black_box;

use calimero_crypto::{Nonce, SharedKey};
use calimero_primitives::identity::PrivateKey;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

const NONCE: Nonce = [7_u8; 12];

fn aead(c: &mut Criterion) {
    let mut rng = rand::thread_rng();
    let sk = PrivateKey::random(&mut rng);
    let key = SharedKey::from_sk(&sk);

    let mut group = c.benchmark_group("aead");

    // 256 B: a governance op. 4 KiB: a typical delta. 64 KiB: libp2p's old
    // gossip ceiling. 1 MiB: a blob chunk and the current gossip ceiling.
    for bytes in [256_usize, 4_096, 65_536, 1_048_576] {
        let plaintext = vec![0xAB_u8; bytes];
        let ciphertext = key
            .encrypt_with_nonce(plaintext.clone(), NONCE)
            .expect("sealing a well-formed payload cannot fail");

        group.throughput(Throughput::Bytes(bytes as u64));

        group.bench_with_input(BenchmarkId::new("encrypt", bytes), &plaintext, |b, pt| {
            b.iter(|| black_box(key.encrypt_with_nonce(pt.clone(), NONCE)));
        });

        group.bench_with_input(BenchmarkId::new("decrypt", bytes), &ciphertext, |b, ct| {
            b.iter(|| black_box(key.decrypt(ct.clone(), NONCE)));
        });
    }

    // Per-peer-pair, not per-message.
    let peer = PrivateKey::random(&mut rng);
    let peer_pk = peer.public_key();
    group.throughput(Throughput::Elements(1));
    group.bench_function("derive_shared_key", |b| {
        b.iter(|| black_box(SharedKey::new(black_box(&sk), black_box(&peer_pk))));
    });

    group.finish();
}

criterion_group!(benches, aead);
criterion_main!(benches);
