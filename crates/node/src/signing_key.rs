//! The node's signing key: the Ed25519 key it signs receipts and session
//! tokens with, attested on its own (`GET /attestation?purpose=receipts`).
//!
//! Made on a node's first start and kept in its data directory ([`FILE`]),
//! wrapped under the node's K_r, so a restart on the same data directory (a
//! crash, a rolling upgrade of the binary) keeps it: the sessions the node
//! issued go on verifying on every node, and its receipts stay checkable
//! against its current attestation. A node replaced on a new data directory
//! makes a new one.
//!
//! Wrapped under K_r, not K_c. K_r comes only to the release whose mero-kms
//! holds its root (`kms.rs`), so only that release opens the key: a node that
//! attested the key once under release R knows that whatever it signs, R
//! signed. K_c is every release's (`cluster_key`), so a node on another
//! release, booted on this disk, could sign as this node under the release the
//! other nodes attested.

use std::path::Path;

use ed25519_dalek::SigningKey;
use eyre::WrapErr;

use crate::cluster_key::{open_secret, seal_secret, write_secret};

/// The file in the data directory that holds the key wrapped under K_r.
pub const FILE: &str = "signing-key";

const MAGIC: &[u8] = b"calimero.signing-key.v1\n";

/// What the wrapping key is derived from K_r with.
const LABEL: &[u8] = b"calimero.signing-key.wrap.v1";

/// This node's signing key from `dir`, made and stored there first if the
/// node has none. A K_r that does not open the stored key stops the node, as
/// it does for the cluster key: that disk belongs to another release.
pub fn load_or_create(dir: &Path, storage_key: &[u8; 32]) -> eyre::Result<SigningKey> {
    let path = dir.join(FILE);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let seed = open_secret(storage_key, LABEL, MAGIC, &bytes).map_err(|e| {
                eyre::eyre!(
                    "this node's storage key does not open its signing key ({e}): the data \
                     directory was written on another mero-tee release"
                )
            })?;
            return Ok(SigningKey::from_bytes(&seed));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).wrap_err_with(|| format!("cannot read {}", path.display())),
    }
    let seed = calimero_identity::random::<32>();
    write_secret(dir, FILE, &seal_secret(storage_key, LABEL, MAGIC, &seed)?)?;
    Ok(SigningKey::from_bytes(&seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_kept_across_starts_and_opens_only_with_its_k_r() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create(dir.path(), &[1; 32]).unwrap();
        let again = load_or_create(dir.path(), &[1; 32]).unwrap();
        assert_eq!(first.to_bytes(), again.to_bytes());
        let e = load_or_create(dir.path(), &[2; 32]).unwrap_err();
        assert!(
            format!("{e:#}").contains("another mero-tee release"),
            "{e:#}"
        );
        let other = tempfile::tempdir().unwrap();
        let fresh = load_or_create(other.path(), &[1; 32]).unwrap();
        assert_ne!(
            first.to_bytes(),
            fresh.to_bytes(),
            "a new data directory, a new key"
        );
    }
}
