// Multi-segment golden vectors for sealed-file export. Produced by the Rust reference: a throwaway
// binary path-depending on core crates/sealed sealed plaintext (byte i = i % 251) with SegmentWriter
// (write, or write_more where `more`) under a random data key and printed the key, segment hashes,
// file id, size and plaintext SHA-256. No ciphertext is stored: the test seals deterministically
// with WebCrypto under the same key and checks every segment hash against these.
export const SEALED_FILE_MULTI_SEGMENT_VECTORS = [
  {
    "name": "2 segs, partial tail",
    "len": 16777221,
    "more": false,
    "key": "bd62add5dd5c6627668c7e5622fb13c3d33d1c199f57c3fec719ce7896bbd475",
    "id": "8d3e6b52e917119758ad8988002615bf8d72915aa22b449393838ae0a4f84b26",
    "sha256": "de44529cece48237d30967c74f61b9541b809af36ede33d34537e06b5bd946a5",
    "size": 16777221,
    "segments": [
      "6d9128647ecb81f44b7a3d7d311846884e0238b38bac369840196ac5eb6cd0c4",
      "396c500e1a6fcddf45920886c5076c394b21d58dee6c24b6fef64b183ea277b5"
    ],
    "lastLen": 21
  },
  {
    "name": "exact 256 chunks, single write",
    "len": 16777216,
    "more": false,
    "key": "a619e3daf3d3b63d923e275366b27a5863193d7da88210e78bda2dd374eb7b16",
    "id": "1fc1460330616abba04cba04d9f6e1958a1952d82c8e62af7289b008d0ff469a",
    "sha256": "287507f403176f1f5b22b9a4d9cb49f7d7f88ac19e406b5ae87ce109564846bd",
    "size": 16777216,
    "segments": [
      "d4abbbee5e1ac0c4f100a3d904dbb7c70b9f10e7bcda91351ba52907ed33fe91"
    ],
    "lastLen": 16781312
  },
  {
    "name": "exact 256 chunks, write_more",
    "len": 16777216,
    "more": true,
    "key": "97aeef4cfabf1ca73736d52d57eb0b32cc479d26e4ea40b79f670001e25e02ab",
    "id": "7f17a95b2efbff20595981f62913293a2d636cde08b4b0746b74e1e83746f280",
    "sha256": "287507f403176f1f5b22b9a4d9cb49f7d7f88ac19e406b5ae87ce109564846bd",
    "size": 16777216,
    "segments": [
      "98f737f704bd8a265a2a3426d03d19eb05fc1cdd4a00dd7d9a115e1c76688e08",
      "4527c69e42bdddeac70a7016a11ee265494351718086e728c6e4f1afc74f8196"
    ],
    "lastLen": 16
  },
  {
    "name": "512 chunks + 1 (3 segs)",
    "len": 33554433,
    "more": false,
    "key": "4d47052bb6f712d25d4dcf9d81083883d68a55c703a7f0de5770b2ccf5dbce4b",
    "id": "292167c05f0e62fe4d2a6b50bd102c1127d34c072a0c0e6a2ef5bcb556e44fed",
    "sha256": "d956fa95d85b3c20642cb65b037ff8821d9c308e28442e88cfb6e79b07609b84",
    "size": 33554433,
    "segments": [
      "d83523f9acd4e04eee8903fa7e2c6b979a2f8b418d7791229edd0c8843052f98",
      "bab26c2f6d67563880c32ffd728665d649c93eaa63664038b8b59b527481c347",
      "3779c5a0a7bc2413fbd095d91eb9c089286d88b9f92fa29439417806b0b003c1"
    ],
    "lastLen": 17
  }
] as const;
