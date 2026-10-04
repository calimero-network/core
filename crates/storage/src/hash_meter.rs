//! The one SHA-256 every module of this crate hashes with.
//!
//! Without the `cost-meter` feature this is a re-export of `sha2::Sha256`, and
//! nothing else exists: no wrapper, no counter, the same code as naming `sha2`
//! directly. With it, [`Sha256`] is a wrapper that also counts the hashes it
//! finishes and the compression blocks they cost, which `tools/storage-cost`
//! reads through [`take`] and gates as a deterministic proxy for CPU — the
//! work that row counts cannot see. The digest is `sha2`'s either way, so no
//! stored byte, wire byte or gas figure depends on the feature.
//!
//! Only `tools/storage-cost` enables the feature. Hash with `Sha256` from here,
//! not from `sha2`, or the hashing goes uncounted.

#[cfg(feature = "cost-meter")]
pub(crate) use metered::Sha256;
#[cfg(feature = "cost-meter")]
pub use metered::{take, HashWork};
pub(crate) use sha2::Digest;
#[cfg(not(feature = "cost-meter"))]
pub(crate) use sha2::Sha256;

#[cfg(feature = "cost-meter")]
mod metered {
    use std::cell::Cell;

    use sha2::digest::{FixedOutput, HashMarker, Output, OutputSizeUser, Update};

    /// SHA-256 work done on this thread since the last [`take`].
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct HashWork {
        /// Hashes finished.
        pub calls: u64,
        /// Compression-function runs those hashes cost: the padded message is
        /// `len + 9` bytes rounded up to 64, so this is what scales with CPU.
        pub blocks: u64,
    }

    thread_local! {
        static WORK: Cell<HashWork> = const { Cell::new(HashWork { calls: 0, blocks: 0 }) };
    }

    /// The work counted on this thread so far, resetting it to zero.
    pub fn take() -> HashWork {
        WORK.with(|work| work.replace(HashWork::default()))
    }

    /// `sha2::Sha256`, plus the length of what it has been fed.
    #[derive(Clone, Default)]
    pub(crate) struct Sha256 {
        inner: sha2::Sha256,
        len: u64,
    }

    impl HashMarker for Sha256 {}

    impl OutputSizeUser for Sha256 {
        type OutputSize = <sha2::Sha256 as OutputSizeUser>::OutputSize;
    }

    impl Update for Sha256 {
        fn update(&mut self, data: &[u8]) {
            self.len += data.len() as u64;
            Update::update(&mut self.inner, data);
        }
    }

    impl FixedOutput for Sha256 {
        fn finalize_into(self, out: &mut Output<Self>) {
            WORK.with(|work| {
                let mut counted = work.get();
                counted.calls += 1;
                // One 0x80 byte and an 8-byte length follow the message.
                counted.blocks += (self.len + 9).div_ceil(64);
                work.set(counted);
            });
            FixedOutput::finalize_into(self.inner, out);
        }
    }

    #[cfg(test)]
    mod tests {
        use sha2::Digest;

        use super::*;

        #[test]
        fn digest_is_sha2s_and_blocks_follow_the_padding() {
            let _ = take();
            for (len, blocks) in [(0, 1), (55, 1), (56, 2), (64, 2), (119, 2), (120, 3)] {
                let data = vec![7_u8; len];
                assert_eq!(
                    Sha256::digest(&data)[..],
                    sha2::Sha256::digest(&data)[..],
                    "the metered hash must be byte-identical to sha2's"
                );
                assert_eq!(take(), HashWork { calls: 1, blocks }, "len {len}");
            }
        }
    }
}

#[cfg(test)]
mod coverage {
    use std::fs;
    use std::path::Path;

    /// A module that hashes with `sha2` directly is invisible to the CPU gate,
    /// so outside tests only this file (and `lib.rs`'s public `exports`, which
    /// this crate never hashes with) may name it.
    #[test]
    fn production_code_hashes_through_the_meter() {
        fn visit(dir: &Path, offenders: &mut Vec<String>) {
            for entry in fs::read_dir(dir).expect("src is readable") {
                let path = entry.expect("src is readable").path();
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if path.is_dir() {
                    if name != "tests" {
                        visit(&path, offenders);
                    }
                } else if path.extension().is_some_and(|ext| ext == "rs")
                    && !matches!(name, "hash_meter.rs" | "lib.rs" | "tests.rs")
                {
                    let source = fs::read_to_string(&path).expect("source is readable");
                    if source.contains("sha2::") {
                        offenders.push(path.display().to_string());
                    }
                }
            }
        }

        let mut offenders = Vec::new();
        visit(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut offenders,
        );
        assert!(
            offenders.is_empty(),
            "hash with `crate::hash_meter::Sha256`, not `sha2`, so tools/storage-cost counts it: {offenders:?}"
        );
    }
}
