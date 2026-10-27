//! The node's storage key K_r, from mero-kms.
//!
//! The KMS client (`calimero-kms-client`) verifies the KMS against the signed
//! release policy and receives a key sealed to this TD. Either the image's boot
//! script fetches the key (`merod-v2 kms disk-key`) and passes the key file
//! (`--storage-key-file`), or merod-v2 fetches it itself (`--kms-url` with
//! `--kms-identity`).
//!
//! The KMS derives a key per identity, so the nodes of one cluster share one
//! disk identity and get one key. K_r is derived from it, bound to the cluster.

use std::path::Path;

use eyre::{bail, WrapErr};
use ring::hkdf;

/// K_r = HKDF-SHA256(KMS key, salt "calimero.cloud.storage.v1", info cluster).
#[must_use]
pub fn derive_storage_key(material: &[u8], cluster: &str) -> [u8; 32] {
    struct Len32;
    impl hkdf::KeyType for Len32 {
        fn len(&self) -> usize {
            32
        }
    }
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"calimero.cloud.storage.v1").extract(material);
    let info = [cluster.as_bytes()];
    let mut out = [0; 32];
    prk.expand(&info, Len32)
        .and_then(|okm| okm.fill(&mut out))
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    out
}

/// Read the key a boot script fetched with `merod-v2 kms disk-key`.
pub fn read_key_file(path: &Path) -> eyre::Result<zeroize::Zeroizing<Vec<u8>>> {
    let bytes = zeroize::Zeroizing::new(
        std::fs::read(path).wrap_err_with(|| format!("cannot read {}", path.display()))?,
    );
    if bytes.len() < 32 {
        bail!(
            "{} holds {} bytes; a KMS key is at least 32",
            path.display(),
            bytes.len()
        );
    }
    Ok(bytes)
}

/// Fetch the KMS key for the cluster's disk identity (created if missing).
pub async fn fetch(kms_url: &str, identity: &Path) -> eyre::Result<zeroize::Zeroizing<Vec<u8>>> {
    let url = kms_url
        .parse()
        .wrap_err_with(|| format!("invalid KMS URL {kms_url}"))?;
    let identity = camino::Utf8Path::from_path(identity)
        .ok_or_else(|| eyre::eyre!("{} is not UTF-8", identity.display()))?;
    calimero_kms_client::disk_key::fetch_disk_key(url, identity, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_storage_key_is_bound_to_the_cluster() {
        let a = derive_storage_key(&[7; 32], "eu1");
        assert_eq!(a, derive_storage_key(&[7; 32], "eu1"));
        assert_ne!(a, derive_storage_key(&[7; 32], "eu2"));
        assert_ne!(a, derive_storage_key(&[8; 32], "eu1"));
    }

    #[test]
    fn a_short_key_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("k");
        std::fs::write(&p, [1; 16]).unwrap();
        assert!(read_key_file(&p).is_err());
        std::fs::write(&p, [1; 32]).unwrap();
        assert_eq!(read_key_file(&p).unwrap().len(), 32);
    }
}
