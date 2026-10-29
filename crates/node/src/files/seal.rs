//! A file as a STREAM of AES-256-GCM chunks under a fresh data key.
//! Layout: chunks concatenated, each `ciphertext || tag`; every chunk but the
//! last holds [`CHUNK_BYTES`] of plaintext.

use eyre::{bail, eyre, Result as EyreResult};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::digest::{Context, SHA256};
use zeroize::{Zeroize, Zeroizing};

/// Plaintext bytes in every chunk but the last.
pub const CHUNK_BYTES: usize = 64 * 1024;
/// The AES-GCM tag closing each chunk.
pub const TAG_BYTES: usize = 16;

const AAD_PREFIX: &[u8] = b"calimero.file.v1"; // AAD of every chunk, then the last-chunk flag
const SEALED_CHUNK_BYTES: usize = CHUNK_BYTES + TAG_BYTES; // a chunk as stored

pub(super) fn cipher(key: &[u8; 32]) -> LessSafeKey {
    // SAFETY: a 32-byte key is an AES-256 key.
    LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).expect("a 32-byte key"))
}

fn nonce(index: u64) -> Nonce {
    let mut bytes = [0u8; NONCE_LEN];
    bytes[4..].copy_from_slice(&index.to_be_bytes());
    Nonce::assume_unique_for_key(bytes)
}

fn aad(last: bool) -> Aad<[u8; AAD_PREFIX.len() + 1]> {
    let mut bytes = [0u8; AAD_PREFIX.len() + 1];
    bytes[..AAD_PREFIX.len()].copy_from_slice(AAD_PREFIX);
    bytes[AAD_PREFIX.len()] = u8::from(last);
    Aad::from(bytes)
}

/// What a finished file adds up to.
pub struct Sealed {
    /// The ciphertext after what `write` returned.
    pub tail: Vec<u8>,
    /// Plaintext bytes.
    pub size: u64,
    /// SHA-256 of the plaintext.
    pub sha256: [u8; 32],
}

/// Encrypts plaintext fed in any split; holds at most one chunk.
pub struct FileSealer {
    key: Zeroizing<[u8; 32]>,
    cipher: LessSafeKey,
    buffer: Zeroizing<Vec<u8>>,
    index: u64,
    size: u64,
    hash: Context,
}

impl FileSealer {
    #[must_use]
    pub fn new() -> Self {
        Self::with_data_key(Zeroizing::new(calimero_identity::random::<32>()))
    }

    #[cfg(test)]
    fn with_key(key: [u8; 32]) -> Self {
        Self::with_data_key(Zeroizing::new(key))
    }

    fn with_data_key(key: Zeroizing<[u8; 32]>) -> Self {
        Self {
            cipher: cipher(&key),
            key,
            buffer: Zeroizing::new(Vec::with_capacity(CHUNK_BYTES + TAG_BYTES)),
            index: 0,
            size: 0,
            hash: Context::new(&SHA256),
        }
    }

    /// The file's data key, to be wrapped before it is stored.
    #[must_use]
    pub fn data_key(&self) -> &[u8; 32] {
        &self.key
    }

    /// Take more plaintext; returns the ciphertext of every chunk it completed.
    pub fn write(&mut self, mut data: &[u8]) -> EyreResult<Vec<u8>> {
        self.hash.update(data);
        self.size += data.len() as u64;
        let mut out = Vec::new();
        while !data.is_empty() {
            // A full chunk waits for more data: it is the last one only if none comes.
            if self.buffer.len() == CHUNK_BYTES {
                self.seal_buffer(false, &mut out)?;
            }
            let take = data.len().min(CHUNK_BYTES - self.buffer.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
        }
        Ok(out)
    }

    /// Seal the last chunk (empty for an empty file).
    pub fn finish(mut self) -> EyreResult<Sealed> {
        let mut tail = Vec::new();
        self.seal_buffer(true, &mut tail)?;
        let digest = self.hash.finish();
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(digest.as_ref());
        Ok(Sealed {
            tail,
            size: self.size,
            sha256,
        })
    }

    fn seal_buffer(&mut self, last: bool, out: &mut Vec<u8>) -> EyreResult<()> {
        self.cipher
            .seal_in_place_append_tag(nonce(self.index), aad(last), &mut *self.buffer)
            .map_err(|_| eyre!("cannot seal chunk {}", self.index))?;
        out.extend_from_slice(&self.buffer);
        self.buffer.clear();
        self.index += 1;
        Ok(())
    }
}

impl Default for FileSealer {
    fn default() -> Self {
        Self::new()
    }
}

/// Decrypts what [`FileSealer`] wrote, ciphertext fed in any split; holds at most one chunk.
pub struct FileOpener {
    cipher: LessSafeKey,
    buffer: Zeroizing<Vec<u8>>,
    index: u64,
    size: u64,
    hash: Context,
}

impl FileOpener {
    #[must_use]
    pub fn new(data_key: &[u8; 32]) -> Self {
        Self {
            cipher: cipher(data_key),
            buffer: Zeroizing::new(Vec::with_capacity(SEALED_CHUNK_BYTES + 1)),
            index: 0,
            size: 0,
            hash: Context::new(&SHA256),
        }
    }

    /// Take more ciphertext; returns the plaintext of every chunk it completed.
    /// The output is authentic but may be a prefix of the file: nothing is final
    /// until [`finish_expecting`](Self::finish_expecting) returns `Ok`.
    pub fn push(&mut self, mut data: &[u8]) -> EyreResult<Vec<u8>> {
        let mut out = Vec::new();
        while !data.is_empty() {
            let take = data.len().min(SEALED_CHUNK_BYTES + 1 - self.buffer.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            // Bytes past a full chunk prove it was not the last.
            if self.buffer.len() > SEALED_CHUNK_BYTES {
                self.open_front(SEALED_CHUNK_BYTES, false, &mut out)?;
            }
        }
        Ok(out)
    }

    /// Open the last chunk and check the whole plaintext against the size and
    /// SHA-256 recorded when it was sealed: GCM does not commit to its key.
    pub fn finish_expecting(mut self, size: u64, sha256: &[u8; 32]) -> EyreResult<Vec<u8>> {
        let last = self.open_last()?;
        if self.size != size || self.hash.finish().as_ref() != sha256 {
            bail!("the file is not the one that was sealed");
        }
        Ok(last)
    }

    /// Open the last chunk; refuses a stream that ended early.
    fn open_last(&mut self) -> EyreResult<Vec<u8>> {
        if self.buffer.len() < TAG_BYTES {
            bail!("the file is truncated");
        }
        let mut out = Vec::new();
        self.open_front(self.buffer.len(), true, &mut out)?;
        Ok(out)
    }

    fn open_front(&mut self, len: usize, last: bool, out: &mut Vec<u8>) -> EyreResult<()> {
        let index = self.index;
        let chunk = &mut self.buffer[..len];
        let plain = self
            .cipher
            .open_in_place(nonce(index), aad(last), chunk)
            .map_err(|_| eyre!("chunk {index} does not open: the file is altered, truncated or under another key"))?;
        out.extend_from_slice(plain);
        self.hash.update(plain);
        self.size += plain.len() as u64;
        chunk.zeroize();
        self.buffer.drain(..len);
        self.index += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;

    const SIZES: [usize; 6] = [
        0,
        1,
        CHUNK_BYTES - 1,
        CHUNK_BYTES,
        CHUNK_BYTES + 1,
        3 * 1024 * 1024 + 512 * 1024,
    ];

    fn plaintext(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn seal(data: &[u8], split: usize) -> (Vec<u8>, [u8; 32], Sealed) {
        let mut sealer = FileSealer::new();
        let mut ciphertext = Vec::new();
        for piece in data.chunks(split.max(1)) {
            ciphertext.extend(sealer.write(piece).unwrap());
        }
        let key = *sealer.data_key();
        let sealed = sealer.finish().unwrap();
        ciphertext.extend_from_slice(&sealed.tail);
        (ciphertext, key, sealed)
    }

    fn open(ciphertext: &[u8], key: &[u8; 32], split: usize) -> EyreResult<Vec<u8>> {
        let mut opener = FileOpener::new(key);
        let mut out = Vec::new();
        for piece in ciphertext.chunks(split.max(1)) {
            out.extend(opener.push(piece)?);
        }
        out.extend(opener.open_last()?);
        Ok(out)
    }

    #[test]
    fn every_size_round_trips_in_odd_splits() {
        for len in SIZES {
            let data = plaintext(len);
            for (write_split, read_split) in [
                (7919, 4093),
                (CHUNK_BYTES, SEALED_CHUNK_BYTES),
                (1 << 20, 3),
            ] {
                let (ciphertext, key, _) = seal(&data, write_split);
                // 3-byte reads of the large file would be slow; only the small ones use them.
                let read_split = if len > CHUNK_BYTES * 2 {
                    read_split.max(4093)
                } else {
                    read_split
                };
                assert_eq!(open(&ciphertext, &key, read_split).unwrap(), data, "{len}");
            }
        }
    }

    #[test]
    fn the_size_and_hash_are_the_plaintexts() {
        for len in SIZES {
            let data = plaintext(len);
            let (_, _, sealed) = seal(&data, 10_000);
            assert_eq!(sealed.size, len as u64);
            assert_eq!(sealed.sha256.as_slice(), Sha256::digest(&data).as_slice());
        }
    }

    #[test]
    fn chunks_have_fixed_boundaries() {
        let (ciphertext, _, _) = seal(&plaintext(2 * CHUNK_BYTES + 5), 1000);
        assert_eq!(ciphertext.len(), 2 * SEALED_CHUNK_BYTES + 5 + TAG_BYTES);
        let (ciphertext, _, _) = seal(&plaintext(CHUNK_BYTES), 1000);
        assert_eq!(ciphertext.len(), SEALED_CHUNK_BYTES);
        let (ciphertext, _, _) = seal(&[], 1);
        assert_eq!(ciphertext.len(), TAG_BYTES);
    }

    #[test]
    fn every_file_gets_its_own_key() {
        assert_ne!(seal(b"a", 1).1, seal(b"a", 1).1);
    }

    #[test]
    fn a_truncated_file_does_not_open() {
        for len in [0, 1, CHUNK_BYTES, 2 * CHUNK_BYTES + 3] {
            let (ciphertext, key, _) = seal(&plaintext(len), 4096);
            for cut in [TAG_BYTES, SEALED_CHUNK_BYTES.min(ciphertext.len())] {
                assert!(
                    open(&ciphertext[..ciphertext.len() - cut], &key, 4096).is_err(),
                    "{len} minus {cut}"
                );
            }
        }
        assert!(open(&[], &[0; 32], 1).is_err());
    }

    #[test]
    fn a_file_cut_at_a_chunk_boundary_does_not_open() {
        let (ciphertext, key, _) = seal(&plaintext(2 * CHUNK_BYTES + 3), 4096);
        let whole_chunks = &ciphertext[..2 * SEALED_CHUNK_BYTES];
        assert!(open(whole_chunks, &key, 4096).is_err());
        assert!(open(&whole_chunks[..SEALED_CHUNK_BYTES], &key, 4096).is_err());
    }

    #[test]
    fn a_file_must_match_the_size_and_hash_it_was_sealed_with() {
        let data = plaintext(CHUNK_BYTES + 10);
        let (ciphertext, key, sealed) = seal(&data, 4096);
        let finish = |size: u64, sha256: &[u8; 32]| {
            let mut opener = FileOpener::new(&key);
            let mut out = opener.push(&ciphertext).unwrap();
            out.extend(opener.finish_expecting(size, sha256)?);
            Ok::<_, eyre::Report>(out)
        };
        assert_eq!(finish(sealed.size, &sealed.sha256).unwrap(), data);
        assert!(finish(sealed.size + 1, &sealed.sha256).is_err());
        assert!(finish(sealed.size, &[0; 32]).is_err());
    }

    #[test]
    fn a_dropped_middle_chunk_does_not_open() {
        let (mut ciphertext, key, _) = seal(&plaintext(3 * CHUNK_BYTES), 4096);
        ciphertext.drain(SEALED_CHUNK_BYTES..2 * SEALED_CHUNK_BYTES);
        assert!(open(&ciphertext, &key, 4096).is_err());
    }

    #[test]
    fn a_sealed_file_matches_its_known_answer() {
        let data = plaintext(CHUNK_BYTES + 3);
        let mut sealer = FileSealer::with_key([7; 32]);
        let mut ciphertext = sealer.write(&data).unwrap();
        let sealed = sealer.finish().unwrap();
        ciphertext.extend_from_slice(&sealed.tail);
        assert_eq!(ciphertext.len(), SEALED_CHUNK_BYTES + 3 + TAG_BYTES);
        assert_eq!(
            hex::encode(Sha256::digest(&ciphertext)),
            "20b93ca7624c2b4b57bd14723f9a5549bda5ffd1b9e7ff65ac3eeaffbbce29b1"
        );
        assert_eq!(
            hex::encode(sealed.sha256),
            "6859d9b53d73fd394a476c8cbf60367e041a0188a670fa853913becda71267fc"
        );
        assert_eq!(open(&ciphertext, &[7; 32], 4096).unwrap(), data);
    }

    #[test]
    fn data_after_the_last_chunk_does_not_open() {
        for len in [0, 10, CHUNK_BYTES] {
            let (mut ciphertext, key, _) = seal(&plaintext(len), 4096);
            ciphertext.extend_from_slice(&[0u8; TAG_BYTES]);
            assert!(open(&ciphertext, &key, 4096).is_err(), "{len}");
            ciphertext.truncate(ciphertext.len() - TAG_BYTES);
            ciphertext.push(0);
            assert!(
                open(&ciphertext, &key, 4096).is_err(),
                "{len} plus one byte"
            );
        }
    }

    #[test]
    fn a_reordered_chunk_does_not_open() {
        let (mut ciphertext, key, _) = seal(&plaintext(3 * CHUNK_BYTES), 4096);
        let (first, rest) = ciphertext.split_at_mut(SEALED_CHUNK_BYTES);
        first.swap_with_slice(&mut rest[..SEALED_CHUNK_BYTES]);
        assert!(open(&ciphertext, &key, 4096).is_err());
    }

    #[test]
    fn an_altered_chunk_does_not_open() {
        let (mut ciphertext, key, _) = seal(&plaintext(2 * CHUNK_BYTES), 4096);
        ciphertext[100] ^= 1;
        assert!(open(&ciphertext, &key, 4096).is_err());
    }

    #[test]
    fn a_wrong_key_does_not_open() {
        let (ciphertext, _, _) = seal(&plaintext(100), 4096);
        assert!(open(&ciphertext, &[9; 32], 4096).is_err());
    }

    #[test]
    fn neither_side_holds_more_than_a_chunk() {
        let data = plaintext(5 * CHUNK_BYTES);
        let mut sealer = FileSealer::new();
        let mut ciphertext = Vec::new();
        for piece in data.chunks(5000) {
            ciphertext.extend(sealer.write(piece).unwrap());
            assert!(sealer.buffer.len() <= CHUNK_BYTES);
        }
        let key = *sealer.data_key();
        ciphertext.extend(sealer.finish().unwrap().tail);
        let mut opener = FileOpener::new(&key);
        for piece in ciphertext.chunks(5000) {
            opener.push(piece).unwrap();
            assert!(opener.buffer.len() <= SEALED_CHUNK_BYTES);
        }
    }
}
