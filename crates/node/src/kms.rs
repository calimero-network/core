//! The node's storage key K_r, from mero-kms.
//!
//! The KMS client (`calimero-kms-client`) verifies the KMS against the signed
//! release policy and receives a key sealed to this TD. A TDX node fetches it
//! itself (`--kms-url` with `--kms-identity`), and only so (`key_source`); the
//! image's boot script uses the same client for the data disk's key (`merod
//! kms disk-key`). `--storage-key`/`--storage-key-file` are development only.
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

/// Read the key a boot script fetched with `merod kms disk-key`.
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

/// Where a node's storage key comes from.
#[derive(Debug, PartialEq, Eq)]
pub enum KeySource<'a> {
    /// A hex key on the command line (development only).
    Raw(&'a str),
    /// A key file (development only).
    File(&'a Path),
    /// mero-kms, after it verifies this TD (`--kms-url` with `--kms-identity`).
    Kms { url: &'a str, identity: &'a Path },
    /// The development key derived from the cluster name.
    Dev,
}

/// Pick the storage key's source. A TDX node (no `--dev`) takes it from
/// mero-kms only: a key the host could hand it (a flag, a file) would let
/// whoever runs the host read the disks (Xil's review, issue 3).
pub fn key_source<'a>(
    dev: bool,
    raw: Option<&'a str>,
    file: Option<&'a Path>,
    kms: Option<(&'a str, &'a Path)>,
) -> eyre::Result<KeySource<'a>> {
    if !dev && (raw.is_some() || file.is_some()) {
        bail!(
            "a TDX node takes its storage key from mero-kms only (--kms-url with \
             --kms-identity); --storage-key and --storage-key-file are for --dev"
        );
    }
    Ok(match (raw, file, kms) {
        (Some(k), _, _) => KeySource::Raw(k),
        (None, Some(f), _) => KeySource::File(f),
        (None, None, Some((url, identity))) => KeySource::Kms { url, identity },
        (None, None, None) if dev => KeySource::Dev,
        (None, None, None) => bail!("no storage key: pass --kms-url with --kms-identity"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Xil's review, issue 3: a TDX-mode node started with `--storage-key` or
    /// a key file (32 zero bytes included), never asking mero-kms.
    #[test]
    fn a_tdx_node_takes_its_key_from_the_kms_only() {
        let id = Path::new("/id");
        let file = Path::new("/zeros");
        assert!(key_source(false, Some(&"00".repeat(32)), None, None).is_err());
        assert!(key_source(false, None, Some(file), Some(("https://kms", id))).is_err());
        assert!(key_source(false, None, None, None).is_err());
        assert_eq!(
            key_source(false, None, None, Some(("https://kms", id))).unwrap(),
            KeySource::Kms {
                url: "https://kms",
                identity: id
            }
        );
        assert_eq!(key_source(true, None, None, None).unwrap(), KeySource::Dev);
        assert_eq!(
            key_source(true, None, Some(file), None).unwrap(),
            KeySource::File(file)
        );
    }

    #[test]
    fn the_storage_key_is_bound_to_the_cluster() {
        let a = derive_storage_key(&[7; 32], "eu1");
        assert_eq!(a, derive_storage_key(&[7; 32], "eu1"));
        assert_ne!(a, derive_storage_key(&[7; 32], "eu2"));
        assert_ne!(a, derive_storage_key(&[8; 32], "eu1"));
    }

    /// K_r for a known KMS key, computed outside this crate (Python's HKDF):
    /// a change here makes every node's store unopenable, so it needs a
    /// migration, not a new vector.
    #[test]
    fn the_storage_key_derivation_is_unchanged() {
        assert_eq!(
            hex::encode(derive_storage_key(&[0x11; 32], "eu1")),
            "54497d225e155f92eab5683043166f13a25d699033b0a5a74c30d8fd4253e0e6"
        );
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
