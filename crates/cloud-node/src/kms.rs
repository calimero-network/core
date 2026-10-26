//! The node's storage key K_r, from mero-kms.
//!
//! merod already holds the KMS client the TEE images use: it verifies the KMS
//! against the signed release policy and receives a key sealed to this TD
//! (`merod kms disk-key`). merod-v2 reuses it rather than repeating it: either
//! the image's boot script runs it and passes the key file (`--storage-key-file`),
//! or merod-v2 runs it itself (`--kms-url` with `--kms-identity`).
//!
//! The KMS derives a key per identity, so the nodes of one cluster share one
//! disk identity and get one key. K_r is derived from it, bound to the cluster.

use std::path::{Path, PathBuf};

use eyre::{bail, WrapErr};
use ring::hkdf;

/// Where `merod kms disk-key` may write: it refuses anything but tmpfs/ramfs.
const KEY_DIR: &str = "/dev/shm";

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

/// Run `merod kms disk-key` and read the key it writes, then remove the file.
/// The release policy comes from the environment merod reads
/// (`MERO_TEE_VERSION` and friends), which the child inherits.
pub async fn fetch_with_merod(
    merod: &Path,
    kms_url: &str,
    identity: &Path,
) -> eyre::Result<zeroize::Zeroizing<Vec<u8>>> {
    fetch_with_merod_in(merod, kms_url, identity, Path::new(KEY_DIR)).await
}

async fn fetch_with_merod_in(
    merod: &Path,
    kms_url: &str,
    identity: &Path,
    dir: &Path,
) -> eyre::Result<zeroize::Zeroizing<Vec<u8>>> {
    let out: PathBuf = dir.join(format!(
        "merod-v2-key-{}",
        hex::encode(calimero_identity::random::<8>())
    ));
    let status = tokio::process::Command::new(merod)
        .args(["kms", "disk-key", "--kms-url", kms_url, "--identity"])
        .arg(identity)
        .arg("--create-identity")
        .arg("--key-out")
        .arg(&out)
        .status()
        .await
        .wrap_err_with(|| format!("cannot run {}", merod.display()))?;
    let key = read_key_file(&out);
    let _removed = std::fs::remove_file(&out);
    if !status.success() {
        bail!("merod kms disk-key failed ({status})");
    }
    key
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

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_merod_and_removes_the_key_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let args_log = dir.path().join("args");
        // A stand-in merod that records its arguments and writes 32 bytes to --key-out.
        let fake = dir.path().join("merod");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\necho \"$@\" > {log}\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = --key-out ]; then printf '%032d' 0 > \"$2\"; fi\n  shift\ndone\n",
                log = args_log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let key = fetch_with_merod_in(&fake, "https://kms.example", Path::new("/id"), dir.path())
            .await
            .unwrap();
        assert_eq!(key.len(), 32);
        let args = std::fs::read_to_string(&args_log).unwrap();
        assert!(args.starts_with("kms disk-key --kms-url https://kms.example --identity /id"));
        // Nothing is left behind.
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("merod-v2-key-"))
            .collect();
        assert!(left.is_empty());
    }
}
