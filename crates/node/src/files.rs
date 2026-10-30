//! Sealed files of component apps: the encrypted stream a file is stored as
//! (`seal`), and the wrapping of its data key under the app's file key and to a
//! job node (`wrap`). Plaintext exists only inside the TD.

use crate::backup::tag;

mod seal;
mod wrap;

pub use seal::{FileOpener, FileSealer, Sealed, CHUNK_BYTES, TAG_BYTES};
pub use wrap::{
    unwrap_for_job, unwrap_key, wrap_for_job, wrap_for_job_derived, wrap_key, JobWrap, WrapKind,
};

/// The stored size of a file of `size` plaintext bytes: a tag per chunk, one for an empty file.
#[must_use]
pub fn sealed_len(size: u64) -> u64 {
    size + size.div_ceil(CHUNK_BYTES as u64).max(1) * TAG_BYTES as u64
}

/// The noise seed of a run: the same for every attempt, so a retry draws the same noise.
#[must_use]
pub fn noise_seed(file_key: &[u8; 32], run_id: &str) -> [u8; 32] {
    tag(file_key, format!("calimero.compute.seed.v1|{run_id}"))
}

/// The capability a job presents for one attempt of a run; any replica can derive it.
#[must_use]
pub fn run_token(file_key: &[u8; 32], run_id: &str, attempt: u32) -> [u8; 32] {
    tag(
        file_key,
        format!("calimero.compute.token.v1|{run_id}|{attempt}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_size_adds_one_tag_per_chunk_and_one_for_an_empty_file() {
        let (chunk, tag) = (CHUNK_BYTES as u64, TAG_BYTES as u64);
        assert_eq!(sealed_len(0), tag);
        assert_eq!(sealed_len(1), 1 + tag);
        assert_eq!(sealed_len(chunk), chunk + tag);
        assert_eq!(sealed_len(chunk + 1), chunk + 1 + 2 * tag);
    }

    #[test]
    fn the_noise_seed_is_fixed_per_run() {
        let key = [1u8; 32];
        assert_eq!(noise_seed(&key, "run-1"), noise_seed(&key, "run-1"));
        assert_ne!(noise_seed(&key, "run-1"), noise_seed(&key, "run-2"));
        assert_ne!(noise_seed(&key, "run-1"), noise_seed(&[2u8; 32], "run-1"));
    }

    #[test]
    fn the_derivations_match_their_known_answers() {
        let key = [1u8; 32];
        assert_eq!(
            hex::encode(noise_seed(&key, "run-1")),
            "1a0d77269a1f690cdc7374b7a0cdf36d82eef519df822eb8c21438061af0405c"
        );
        assert_eq!(
            hex::encode(run_token(&key, "run-1", 2)),
            "d1bdcadf164c9ea0c919f86f637e464de4a01bf9ea4f25fd81ee2a6154ebe874"
        );
    }

    #[test]
    fn the_run_token_is_fixed_per_attempt() {
        let key = [1u8; 32];
        assert_eq!(run_token(&key, "run-1", 0), run_token(&key, "run-1", 0));
        assert_ne!(run_token(&key, "run-1", 0), run_token(&key, "run-1", 1));
        assert_ne!(run_token(&key, "run-1", 0), run_token(&key, "run-2", 0));
    }
}
