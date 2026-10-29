//! `merod kms disk-key`: fetch the key that unlocks a TEE node's data disk.
//!
//! merod's own datastore is encrypted from `init --kms-url`, but a data disk
//! holds more than the datastore — blobs, `config.toml`, the TLS key, the fleet
//! sidecar's state — and TDX protects the VM's memory, not its disks. The node
//! image therefore encrypts the whole data disk (LUKS2 with integrity), and this
//! command is how its boot script obtains the key before the disk is mounted.
//!
//! The key comes from mero-kms exactly as the storage key does: the KMS
//! releases it only to a TD whose quote matches the signed release policy and
//! is bound to the requesting identity. The identity is a dedicated keypair,
//! NOT the node's libp2p identity — that one lives on the very disk being
//! unlocked, and a distinct identity also gives the disk its own derivation
//! (`{namespace}/{profile}/{peerId}`) rather than reusing the store's key.
//!
//! The identity file may sit somewhere the host can read (the boot script keeps
//! it in the LUKS2 header): it obtains nothing without a genuine TD to answer the
//! KMS's attestation challenge. The KEY must not, so it is written only to a
//! RAM-backed filesystem, never to stdout — merod's logs go to stdout.
//!
//! With `--evidence-out` it also writes what it verified of the KMS (a
//! [`KmsEvidence`], JSON) to a RAM-backed file, for `merod run
//! --kms-disk-evidence` to report in the node's attestation. Not a secret, but
//! the node vouches for it, so it lives where only this TD can write it.

use std::io::Write;
use std::path::Path;

use crate::config::TeeConfig;
use calimero_tee_attestation::KmsEvidence;
use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser;
use eyre::{bail, Result as EyreResult, WrapErr};
use libp2p_identity::Keypair;
use tracing::info;
use url::Url;

/// Filesystems whose contents live only in memory. On a TD that memory is
/// TDX-encrypted, so a key written here never reaches a host-readable device.
const RAM_FILESYSTEMS: &[&str] = &["tmpfs", "ramfs"];

/// The largest evidence file [`read_evidence`] reads: a record is a few
/// hundred bytes, so anything near this is not one.
pub const MAX_EVIDENCE_BYTES: u64 = 16 * 1024;

/// Fetch the data-disk unlock key for this TD from the KMS.
#[derive(Debug, Parser)]
pub struct KmsDiskKeyCommand {
    /// URL of the mero-kms service.
    #[arg(long, value_name = "URL")]
    kms_url: Url,

    /// The disk-unlock identity: a protobuf-encoded libp2p keypair.
    #[arg(long, value_name = "PATH")]
    identity: Utf8PathBuf,

    /// Generate the identity first if the file does not exist. For a disk being
    /// formatted; an existing disk must present the identity it was made with.
    #[arg(long)]
    create_identity: bool,

    /// Where to write the key. Must not exist yet, and must be on tmpfs/ramfs.
    #[arg(long, value_name = "PATH")]
    key_out: Utf8PathBuf,

    /// Also write what was verified of the KMS (JSON) here, for `merod run
    /// --kms-disk-evidence`. Must not exist yet, and must be on tmpfs/ramfs.
    #[arg(long, value_name = "PATH")]
    evidence_out: Option<Utf8PathBuf>,
}

impl KmsDiskKeyCommand {
    pub async fn run(self) -> EyreResult<()> {
        // Checked before anything touches the network, so a misconfigured call
        // cannot fetch a key it would then have nowhere safe to put.
        ensure_on_ram_filesystem(&self.key_out)?;
        if let Some(path) = &self.evidence_out {
            ensure_on_ram_filesystem(path)?;
        }

        let (key, evidence) =
            fetch_disk_key(self.kms_url, &self.identity, self.create_identity).await?;
        write_private_new(&self.key_out, &key)?;
        info!(key_out = %self.key_out, "Wrote the data-disk key");
        if let Some(path) = &self.evidence_out {
            let json = serde_json::to_vec(&evidence).wrap_err("could not encode the evidence")?;
            write_private_new(path, &json)?;
            info!(evidence_out = %path, "Wrote what was verified of the KMS");
        }
        Ok(())
    }
}

/// Fetch the data-disk key for `identity` (created first if `create` and
/// missing) from the KMS at `kms_url`, verified against the signed release
/// policy the environment names (`MERO_TEE_VERSION` and friends). Answers the
/// key and the record of the KMS quote that policy accepted.
pub async fn fetch_disk_key(
    kms_url: Url,
    identity: &Utf8Path,
    create: bool,
) -> EyreResult<(zeroize::Zeroizing<Vec<u8>>, KmsEvidence)> {
    let identity = load_or_create_identity(identity, create)?;

    // The same rule as merod's `init --kms-url`: without the signed release
    // policy the KMS goes unverified, and a key from an unverified KMS may be
    // known to whoever runs the endpoint.
    let Some(policy) = crate::policy::resolve_policy().await? else {
        bail!(
            "disk-key needs the signed mero-tee release policy to verify the KMS, and no \
             release is named. Set MERO_TEE_VERSION (or MERO_KMS_VERSION / \
             MERO_KMS_RELEASE_TAG)."
        );
    };

    let tee = TeeConfig::kms(kms_url);
    let peer_id = identity.public().to_peer_id().to_base58();
    info!(%peer_id, "Fetching the data-disk key from the KMS");
    let released =
        crate::kms::fetch_storage_key(tee.kms.as_ref(), &peer_id, &identity, Some(&policy))
            .await
            .wrap_err("could not fetch the data-disk key from the KMS")?;
    let key = zeroize::Zeroizing::new(released.key);
    let evidence = released
        .evidence
        .ok_or_else(|| eyre::eyre!("the release policy verified the KMS but left no record"))?;
    Ok((key, evidence))
}

/// Read what `kms disk-key --evidence-out` recorded of the KMS. Refused unless
/// the file is on a RAM-backed filesystem (inside a TD, nothing outside it can
/// have written it), at most [`MAX_EVIDENCE_BYTES`], and exactly one record.
pub fn read_evidence(path: &Path) -> EyreResult<KmsEvidence> {
    let path =
        Utf8Path::from_path(path).ok_or_else(|| eyre::eyre!("{} is not UTF-8", path.display()))?;
    ensure_on_ram_filesystem(path)?;
    parse_evidence_file(path)
}

fn parse_evidence_file(path: &Utf8Path) -> EyreResult<KmsEvidence> {
    let size = std::fs::metadata(path)
        .wrap_err_with(|| format!("cannot read {path}"))?
        .len();
    if size > MAX_EVIDENCE_BYTES {
        bail!("{path} holds {size} bytes; a KMS evidence record is at most {MAX_EVIDENCE_BYTES}");
    }
    let bytes = std::fs::read(path).wrap_err_with(|| format!("cannot read {path}"))?;
    serde_json::from_slice(&bytes).wrap_err_with(|| format!("{path} is not a KMS evidence record"))
}

/// The libp2p peer id of the identity at `path`, which must exist: the id
/// mero-kms releases that identity's key under. Reads the file, never creates it.
pub fn identity_peer_id(path: &Path) -> EyreResult<String> {
    let path =
        Utf8Path::from_path(path).ok_or_else(|| eyre::eyre!("{} is not UTF-8", path.display()))?;
    Ok(load_or_create_identity(path, false)?
        .public()
        .to_peer_id()
        .to_base58())
}

fn load_or_create_identity(path: &Utf8Path, create: bool) -> EyreResult<Keypair> {
    match std::fs::read(path) {
        Ok(bytes) => Keypair::from_protobuf_encoding(&bytes)
            .wrap_err_with(|| format!("{path} is not a protobuf-encoded libp2p keypair")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound && create => {
            let keypair = Keypair::generate_ed25519();
            let encoded = zeroize::Zeroizing::new(
                keypair
                    .to_protobuf_encoding()
                    .wrap_err("could not encode the new disk identity")?,
            );
            write_private_new(path, &encoded)?;
            info!(%path, "Generated a new disk-unlock identity file");
            Ok(keypair)
        }
        Err(err) => Err(err).wrap_err_with(|| format!("could not read the disk identity {path}")),
    }
}

/// Create `path` owner-only and write `bytes`, refusing to replace a file.
fn write_private_new(path: &Utf8Path, bytes: &[u8]) -> EyreResult<()> {
    let mut options = std::fs::OpenOptions::new();
    let _ = options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .wrap_err_with(|| format!("could not create {path} (it must not already exist)"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .wrap_err_with(|| format!("could not write {path}"))
}

/// Refuse unless `path`'s directory is on a RAM-backed filesystem.
fn ensure_on_ram_filesystem(path: &Utf8Path) -> EyreResult<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_str().is_empty())
        .unwrap_or(Utf8Path::new("."));
    let dir = dir
        .canonicalize_utf8()
        .wrap_err_with(|| format!("{dir} does not exist"))?;
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")
        .wrap_err("cannot read /proc/self/mountinfo to check where the key would be written")?;
    let fstype = filesystem_of(&mountinfo, &dir).unwrap_or("unknown");
    if !RAM_FILESYSTEMS.contains(&fstype) {
        bail!(
            "refusing to write the data-disk key to {path}: {dir} is on {fstype}, not tmpfs or \
             ramfs. The key must never reach a device the host can read."
        );
    }
    Ok(())
}

/// The filesystem type of the longest mount point containing `dir`.
///
/// `/proc/self/mountinfo` lines are
/// `id parent maj:min root mount-point options [optional...] - fstype source super`,
/// with spaces in paths escaped as `\040`.
fn filesystem_of<'a>(mountinfo: &'a str, dir: &Utf8Path) -> Option<&'a str> {
    mountinfo
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let mount_point = fields.nth(4)?.replace("\\040", " ");
            let fstype = line.split(" - ").nth(1)?.split(' ').next()?;
            dir.starts_with(&mount_point)
                .then_some((mount_point.len(), fstype))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, fstype)| fstype)
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;

    use super::{filesystem_of, identity_peer_id, parse_evidence_file, MAX_EVIDENCE_BYTES};

    const MOUNTINFO: &str = "\
22 1 8:1 / / rw,relatime - ext4 /dev/sda1 rw
23 22 0:21 / /run rw,nosuid - tmpfs tmpfs rw,size=1024k
24 22 8:17 / /mnt/data rw,relatime - ext4 /dev/mapper/calimero-data rw
25 23 0:22 / /run/calimero\\040keys rw - tmpfs tmpfs rw
";

    /// The deepest mount point wins: `/run/calimero` is tmpfs even though `/`
    /// is ext4, and `/mnt/data` is the (encrypted, but still a DEVICE) disk.
    #[test]
    fn the_deepest_mount_decides_the_filesystem() {
        assert_eq!(
            filesystem_of(MOUNTINFO, Utf8Path::new("/run/calimero")),
            Some("tmpfs")
        );
        assert_eq!(
            filesystem_of(MOUNTINFO, Utf8Path::new("/mnt/data/tls")),
            Some("ext4")
        );
        assert_eq!(
            filesystem_of(MOUNTINFO, Utf8Path::new("/var/lib")),
            Some("ext4")
        );
        assert_eq!(
            filesystem_of(MOUNTINFO, Utf8Path::new("/run/calimero keys/x")),
            Some("tmpfs")
        );
    }

    /// A path-prefix that is not a path component must not count: `/runx` is
    /// not under `/run`.
    #[test]
    fn a_name_prefix_is_not_a_parent_directory() {
        assert_eq!(
            filesystem_of(MOUNTINFO, Utf8Path::new("/runx")),
            Some("ext4")
        );
    }

    /// The evidence a node reads back is exactly the record disk-key wrote,
    /// and nothing that only looks like one.
    #[test]
    fn an_evidence_file_holds_one_record_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().join("evidence.json")).unwrap();
        let record = serde_json::json!({
            "kmsHost": "kms.internal:8080",
            "release": "2.4.1",
            "profile": "locked-read-only",
            "mrtd": "aa".repeat(48),
            "rtmr0": "00".repeat(48),
            "rtmr1": "11".repeat(48),
            "rtmr2": "22".repeat(48),
            "rtmr3": "33".repeat(48),
            "tcbStatus": "UpToDate",
            "quoteSha256": "44".repeat(32),
            "verifiedAt": 1_790_000_000_u64,
        });
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        let evidence = parse_evidence_file(&path).unwrap();
        assert_eq!(evidence.release, "2.4.1");
        assert_eq!(serde_json::to_value(&evidence).unwrap(), record);

        let mut extra = record.clone();
        extra["note"] = serde_json::json!("from the host");
        std::fs::write(&path, serde_json::to_vec(&extra).unwrap()).unwrap();
        assert!(parse_evidence_file(&path).is_err(), "an unknown field");

        let padding = usize::try_from(MAX_EVIDENCE_BYTES).unwrap() + 1;
        std::fs::write(&path, vec![b' '; padding]).unwrap();
        assert!(parse_evidence_file(&path).is_err(), "an oversized file");
    }

    /// `merod attest --identity` names an identity's peer id without ever
    /// creating one.
    #[test]
    fn the_peer_id_of_an_identity_file_is_read_never_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity");
        assert!(identity_peer_id(&path).is_err());
        assert!(!path.exists());
        let keypair = libp2p_identity::Keypair::generate_ed25519();
        std::fs::write(&path, keypair.to_protobuf_encoding().unwrap()).unwrap();
        assert_eq!(
            identity_peer_id(&path).unwrap(),
            keypair.public().to_peer_id().to_base58()
        );
    }
}
