//! `merod kms disk-key`: fetch the key that unlocks a TEE node's data disk.
//!
//! merod's own datastore is encrypted from `init --kms-url`, but a data disk
//! holds more than the datastore — blobs, `config.toml`, the TLS key, the fleet
//! sidecar's state — and TDX protects the VM's memory, not its disks. The node
//! image therefore encrypts the whole data disk (LUKS2 with integrity), and this
//! command is how its boot script obtains the key before the disk is mounted.
//!
//! The key comes from mero-kms-phala exactly as the storage key does: the KMS
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

use std::io::Write;

use calimero_config::TeeConfig;
use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser;
use eyre::{bail, Result as EyreResult, WrapErr};
use libp2p::identity::Keypair;
use tracing::info;
use url::Url;

/// Filesystems whose contents live only in memory. On a TD that memory is
/// TDX-encrypted, so a key written here never reaches a host-readable device.
const RAM_FILESYSTEMS: &[&str] = &["tmpfs", "ramfs"];

/// Fetch the data-disk unlock key for this TD from the KMS.
#[derive(Debug, Parser)]
pub struct KmsDiskKeyCommand {
    /// URL of the mero-kms-phala service.
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
}

impl KmsDiskKeyCommand {
    pub async fn run(self) -> EyreResult<()> {
        // Checked before anything touches the network, so a misconfigured call
        // cannot fetch a key it would then have nowhere safe to put.
        ensure_on_ram_filesystem(&self.key_out)?;

        let identity = load_or_create_identity(&self.identity, self.create_identity)?;

        // The same rule as `init --kms-url`: without the signed release policy
        // the KMS goes unverified, and a key from an unverified KMS may be known
        // to whoever runs the endpoint.
        let Some(policy) = crate::kms_policy::resolve_policy().await? else {
            bail!(
                "disk-key needs the signed mero-tee release policy to verify the KMS, and no \
                 release is named. Set MERO_TEE_VERSION (or MERO_KMS_VERSION / \
                 MERO_KMS_RELEASE_TAG)."
            );
        };

        let tee = TeeConfig::phala(self.kms_url);
        let peer_id = identity.public().to_peer_id().to_base58();
        info!(%peer_id, "Fetching the data-disk key from the KMS");
        let key = zeroize::Zeroizing::new(
            crate::kms::fetch_storage_key(&tee.kms, &peer_id, &identity, Some(&policy))
                .await
                .wrap_err("could not fetch the data-disk key from the KMS")?,
        );

        write_private_new(&self.key_out, &key)?;
        info!(key_out = %self.key_out, "Wrote the data-disk key");
        Ok(())
    }
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
            info!(identity = %path, "Generated a new disk-unlock identity");
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

    use super::filesystem_of;

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
}
