//! What does AES-256-GCM cost at the sizes this system actually moves?

use std::hint::black_box;

use calimero_crypto::{Nonce, SharedKey};
use calimero_primitives::identity::PrivateKey;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

const NONCE: Nonce = [7_u8; 12];

fn aead(c: &mut Criterion) {
    let mut rng = rand::rng();
    let sk = PrivateKey::random(&mut rng);
    let key = SharedKey::from_sk(&sk);

    let mut group = c.benchmark_group("aead");

    // A governance op, a typical delta, the old gossip cap, a blob chunk.
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
