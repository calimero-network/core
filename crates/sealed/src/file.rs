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
const SEGMENT_DOMAIN: &[u8] = b"calimero.file.segments.v1\0"; // prefix of a file id's preimage

/// Sealed chunks in every segment but the last.
pub const SEGMENT_CHUNKS: usize = 256;
/// Plaintext bytes of a segment's worth of chunks: the part size of an upload.
pub const PART_BYTES: usize = SEGMENT_CHUNKS * CHUNK_BYTES;
/// Stored bytes of every segment but the last (16,781,312).
pub const SEGMENT_BYTES: usize = SEGMENT_CHUNKS * SEALED_CHUNK_BYTES;
const _: () = assert!(SEGMENT_BYTES <= crate::server::MAX_SEALED_BYTES); // a segment fits one sealed request

pub fn cipher(key: &[u8; 32]) -> LessSafeKey {
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
    /// Bytes of the current partial chunk the sealer holds.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

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

    /// [`Self::write`] for data after which more follows: a full chunk is sealed too, as it is
    /// not the last.
    pub fn write_more(&mut self, data: &[u8]) -> EyreResult<Vec<u8>> {
        let mut out = self.write(data)?;
        if self.buffer.len() == CHUNK_BYTES {
            self.seal_buffer(false, &mut out)?;
        }
        Ok(out)
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

/// One segment of a sealed file, with its SHA-256.
pub struct Segment {
    pub hash: [u8; 32],
    pub bytes: Vec<u8>,
}

/// A finished file: its last segment (the rest after what `write` returned) and what the mark records.
pub struct SegmentedFile {
    pub last: Segment,
    /// Every segment's hash, in order.
    pub segments: Vec<[u8; 32]>,
    pub id: [u8; 32],
    /// Plaintext bytes.
    pub size: u64,
    /// SHA-256 of the plaintext.
    pub sha256: [u8; 32],
}

/// The file id: a hash over the ordered segment hashes.
#[must_use]
pub fn file_id(segments: &[[u8; 32]]) -> [u8; 32] {
    let mut preimage = SEGMENT_DOMAIN.to_vec();
    preimage.extend_from_slice(
        &u32::try_from(segments.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    preimage.extend(segments.concat());
    calimero_identity::sha256(&preimage)
}

fn segment_of(bytes: Vec<u8>) -> Segment {
    Segment {
        hash: calimero_identity::sha256(&bytes),
        bytes,
    }
}

/// Seals plaintext fed in any split and cuts the ciphertext into segments; holds at most one partial segment.
pub struct SegmentWriter {
    sealer: FileSealer,
    pending: Vec<u8>,
    segments: Vec<[u8; 32]>,
}

impl SegmentWriter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sealer: FileSealer::new(),
            pending: Vec::new(),
            segments: Vec::new(),
        }
    }

    /// The file's data key, to be wrapped before it is stored.
    #[must_use]
    pub fn data_key(&self) -> &[u8; 32] {
        self.sealer.data_key()
    }

    /// Bytes the writer holds: the partial segment and the sealer's partial chunk.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.pending.len() + self.sealer.buffered()
    }

    /// Take more plaintext; returns every segment it completed.
    pub fn write(&mut self, data: &[u8]) -> EyreResult<Vec<Segment>> {
        let sealed = self.sealer.write(data)?;
        Ok(self.cut(&sealed))
    }

    /// [`Self::write`] for a part after which more data follows: a full last chunk is sealed too,
    /// so a part of whole chunks closes its segments at once.
    pub fn write_more(&mut self, data: &[u8]) -> EyreResult<Vec<Segment>> {
        let sealed = self.sealer.write_more(data)?;
        Ok(self.cut(&sealed))
    }

    fn cut(&mut self, sealed: &[u8]) -> Vec<Segment> {
        self.pending.extend_from_slice(sealed);
        let mut done = Vec::new();
        // The sealer withholds the file's last chunk, so a full segment here is never the last.
        let full = self.pending.len() / SEGMENT_BYTES * SEGMENT_BYTES;
        for piece in self.pending[..full].chunks(SEGMENT_BYTES) {
            let segment = segment_of(piece.to_vec());
            self.segments.push(segment.hash);
            done.push(segment);
        }
        self.pending.drain(..full);
        done
    }

    /// Seal the last chunk and close the last segment.
    pub fn finish(mut self) -> EyreResult<SegmentedFile> {
        let sealed = self.sealer.finish()?;
        self.pending.extend_from_slice(&sealed.tail);
        let last = segment_of(self.pending);
        self.segments.push(last.hash);
        Ok(SegmentedFile {
            id: file_id(&self.segments),
            segments: self.segments,
            last,
            size: sealed.size,
            sha256: sealed.sha256,
        })
    }
}

impl Default for SegmentWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// Joins the segments of a file in order, checking each against the list, and opens them.
pub struct SegmentReader {
    opener: FileOpener,
    segments: Vec<[u8; 32]>,
    next: usize,
}

impl SegmentReader {
    #[must_use]
    pub fn new(data_key: &[u8; 32], segments: Vec<[u8; 32]>) -> Self {
        Self {
            opener: FileOpener::new(data_key),
            segments,
            next: 0,
        }
    }

    /// Take the next segment; returns the plaintext of every chunk it completed.
    pub fn push(&mut self, segment: &[u8]) -> EyreResult<Vec<u8>> {
        let Some(want) = self.segments.get(self.next) else {
            bail!("the file has no segment {}", self.next);
        };
        if self.next + 1 < self.segments.len() && segment.len() != SEGMENT_BYTES {
            bail!("segment {} is not a whole segment", self.next);
        }
        if calimero_identity::sha256(segment) != *want {
            bail!("segment {} is not the one the file lists", self.next);
        }
        self.next += 1;
        self.opener.push(segment)
    }

    /// Open the last chunk and prove the plaintext size and SHA-256; refuses a missing segment.
    pub fn finish_expecting(self, size: u64, sha256: &[u8; 32]) -> EyreResult<Vec<u8>> {
        if self.next != self.segments.len() {
            bail!("the file is missing segments");
        }
        self.opener.finish_expecting(size, sha256)
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

    /// 2.5 segments of plaintext, so the file is three segments.
    const SEGMENTED_LEN: usize = SEGMENT_CHUNKS * CHUNK_BYTES * 5 / 2;

    struct Made {
        segments: Vec<Vec<u8>>,
        file: SegmentedFile,
        key: [u8; 32],
    }

    fn make(data: &[u8], split: usize) -> Made {
        let mut writer = SegmentWriter::new();
        let mut segments = Vec::new();
        for piece in data.chunks(split.max(1)) {
            segments.extend(writer.write(piece).unwrap().into_iter().map(|s| s.bytes));
        }
        let key = *writer.data_key();
        let file = writer.finish().unwrap();
        segments.push(file.last.bytes.clone());
        Made {
            segments,
            file,
            key,
        }
    }

    fn join(made: &Made, segments: &[Vec<u8>]) -> EyreResult<Vec<u8>> {
        let mut reader = SegmentReader::new(&made.key, made.file.segments.clone());
        let mut out = Vec::new();
        for segment in segments {
            out.extend(reader.push(segment)?);
        }
        out.extend(reader.finish_expecting(made.file.size, &made.file.sha256)?);
        Ok(out)
    }

    #[test]
    fn two_and_a_half_segments_of_data_make_three_segments_of_whole_chunks() {
        let data = plaintext(SEGMENTED_LEN);
        let made = make(&data, 1 << 20);
        let lens: Vec<usize> = made.segments.iter().map(Vec::len).collect();
        assert_eq!(SEGMENT_BYTES, 16_781_312);
        assert_eq!(lens.len(), 3);
        assert_eq!(lens[..2], [SEGMENT_BYTES; 2]);
        assert_eq!(lens[2], SEGMENT_BYTES / 2);
        assert_eq!(made.file.segments.len(), 3);
        for (segment, hash) in made.segments.iter().zip(&made.file.segments) {
            assert_eq!(Sha256::digest(segment).as_slice(), hash);
        }
    }

    #[test]
    fn cutting_ignores_the_split_the_writer_was_fed() {
        let data = plaintext(SEGMENTED_LEN);
        let key = [5; 32];
        let cut = |split: usize| {
            let mut writer = SegmentWriter {
                sealer: FileSealer::with_key(key),
                ..SegmentWriter::new()
            };
            let mut out: Vec<Vec<u8>> = Vec::new();
            for piece in data.chunks(split) {
                out.extend(writer.write(piece).unwrap().into_iter().map(|s| s.bytes));
            }
            out.push(writer.finish().unwrap().last.bytes);
            out
        };
        assert_eq!(cut(7919), cut(CHUNK_BYTES * 3));
    }

    #[test]
    fn a_file_of_exactly_one_full_segment_is_one_segment() {
        let made = make(&plaintext(SEGMENT_CHUNKS * CHUNK_BYTES), 1 << 20);
        assert_eq!(made.segments.len(), 1);
        assert_eq!(made.segments[0].len(), SEGMENT_BYTES);
        let made = make(&plaintext(SEGMENT_CHUNKS * CHUNK_BYTES + 1), 1 << 20);
        assert_eq!(made.segments.len(), 2);
        assert_eq!(made.segments[1].len(), 1 + TAG_BYTES);
        let made = make(&[], 1);
        assert_eq!(made.segments.len(), 1);
        assert_eq!(made.segments[0].len(), TAG_BYTES);
    }

    #[test]
    fn the_reader_joins_the_segments_back() {
        let data = plaintext(SEGMENTED_LEN);
        let made = make(&data, 1 << 20);
        assert_eq!(join(&made, &made.segments).unwrap(), data);
        let small = make(&plaintext(10), 4);
        assert_eq!(join(&small, &small.segments).unwrap(), plaintext(10));
    }

    #[test]
    fn a_flipped_byte_in_any_segment_fails() {
        let made = make(&plaintext(SEGMENTED_LEN), 1 << 20);
        for k in 0..made.segments.len() {
            let mut segments = made.segments.clone();
            segments[k][1000] ^= 1;
            assert!(join(&made, &segments).is_err(), "segment {k}");
        }
    }

    #[test]
    fn a_swapped_pair_fails() {
        let made = make(&plaintext(SEGMENTED_LEN), 1 << 20);
        let mut segments = made.segments.clone();
        segments.swap(0, 1);
        assert!(join(&made, &segments).is_err());
    }

    #[test]
    fn a_dropped_last_segment_fails() {
        let made = make(&plaintext(SEGMENTED_LEN), 1 << 20);
        assert!(join(&made, &made.segments[..2]).is_err());
    }

    #[test]
    fn an_extra_segment_fails() {
        let made = make(&plaintext(10), 4);
        let mut segments = made.segments.clone();
        segments.push(made.segments[0].clone());
        assert!(join(&made, &segments).is_err());
    }

    #[test]
    fn the_id_changes_with_any_list_change() {
        let a = [[1u8; 32], [2; 32], [3; 32]];
        let id = file_id(&a);
        assert_eq!(id, file_id(&a));
        assert_ne!(id, file_id(&a[..2]));
        assert_ne!(id, file_id(&[a[1], a[0], a[2]]));
        assert_ne!(id, file_id(&[a[0], a[1], [4; 32]]));
        assert_ne!(id, file_id(&[a[0], a[1], a[2], a[2]]));
        assert_ne!(file_id(&a[..1]), a[0]);
    }

    #[test]
    fn one_write_of_many_segments_makes_the_segments_many_small_writes_do() {
        let data = plaintext(SEGMENTED_LEN);
        let small = make(&data, 1 << 20);
        let whole = make(&data, data.len());
        let lens = |m: &Made| m.segments.iter().map(Vec::len).collect::<Vec<_>>();
        assert_eq!(lens(&whole), lens(&small));
        assert_eq!(join(&whole, &whole.segments).unwrap(), data);
    }

    #[test]
    fn the_id_matches_its_known_answer() {
        let mut preimage = b"calimero.file.segments.v1\0".to_vec();
        preimage.extend_from_slice(&2u32.to_be_bytes());
        preimage.extend_from_slice(&[1; 32]);
        preimage.extend_from_slice(&[2; 32]);
        assert_eq!(
            file_id(&[[1; 32], [2; 32]]).as_slice(),
            Sha256::digest(&preimage).as_slice()
        );
    }

    #[test]
    fn the_writer_holds_at_most_one_segment() {
        let mut writer = SegmentWriter::new();
        for piece in plaintext(SEGMENTED_LEN).chunks(1 << 20) {
            writer.write(piece).unwrap();
            assert!(writer.pending.len() <= SEGMENT_BYTES);
        }
    }

    #[test]
    fn a_full_part_closes_its_segment_and_the_file_is_the_one_a_single_write_makes() {
        const PART: usize = SEGMENT_CHUNKS * CHUNK_BYTES;
        let data = plaintext(2 * PART + 1000);
        let writer = || SegmentWriter {
            sealer: FileSealer::with_key([5; 32]),
            ..SegmentWriter::new()
        };
        let mut parted = writer();
        let mut segments = Vec::new();
        for part in data[..2 * PART].chunks(PART) {
            let done = parted.write_more(part).unwrap();
            assert_eq!(done.len(), 1, "a full part closes one segment at once");
            assert_eq!(done[0].bytes.len(), SEGMENT_BYTES);
            assert_eq!(parted.buffered(), 0);
            segments.extend(done.into_iter().map(|s| s.bytes));
        }
        assert!(parted.write(&data[2 * PART..]).unwrap().is_empty());
        segments.push(parted.finish().unwrap().last.bytes);

        let mut single = writer();
        let mut whole: Vec<Vec<u8>> = (single.write(&data).unwrap().into_iter())
            .map(|s| s.bytes)
            .collect();
        whole.push(single.finish().unwrap().last.bytes);
        assert_eq!(segments, whole);
    }
}
