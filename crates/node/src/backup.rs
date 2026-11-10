//! Encrypted app snapshots in object storage (a directory stands in for a
//! bucket), and the keys the cluster derives per app from its cluster key K_c
//! (`crate::cluster_key`).
//!
//! A backup is the app group's Raft snapshot file as the log wrote it, sealed
//! frame by frame under K_c, beside its manifest ([`Manifest`]): which app,
//! group and index it holds, the file's SHA-256, and the release and backup
//! format that wrote it, sealed under a key derived from K_c and bound to the
//! app and the file name. So a backup opens, and restores, only on a node that
//! holds this cluster's K_c, and a file moved under another app's name or
//! swapped for another is refused ([`find`]).

use std::io::Read;
use std::path::{Path, PathBuf};

use calimero_log::store::Sealer;
use ring::hmac;
use serde::{Deserialize, Serialize};

/// The app's TEE key: what values sealed to the app's TEE (`TeeSecret`, an
/// `#[app::tee]` run's own state) open with. The cluster is the app's TEE, so
/// every node derives the same key from K_c, and nothing outside it can.
#[must_use]
pub fn tee_key(cluster_key: &[u8; 32], app_id: &str) -> [u8; 32] {
    tag(cluster_key, format!("calimero.tee.v1|{app_id}"))
}

/// The key that stands for `account`'s devices in `app`: a member's share is
/// sealed to it, not to each device; the member reads through the node,
/// which opens it only in that member's own runs.
#[must_use]
pub fn member_key(cluster_key: &[u8; 32], app_id: &str, account: &str) -> [u8; 32] {
    tag(
        cluster_key,
        format!("calimero.member.v1|{app_id}|{account}"),
    )
}

/// The key the app's files are wrapped and its compute runs are derived under:
/// separate from the TEE key, which is also a curve private key.
#[must_use]
pub fn file_key(cluster_key: &[u8; 32], app_id: &str) -> [u8; 32] {
    tag(cluster_key, format!("calimero.files.v1|{app_id}"))
}

/// The cluster's identity: an Ed25519 key only a node holding the cluster key can make, so its
/// signature proves a node of this cluster signed. Its public half names the cluster to job nodes.
#[must_use]
pub fn cluster_identity_key(cluster_key: &[u8; 32]) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&tag(cluster_key, "calimero.cluster-identity.v1"))
}

/// `HMAC-SHA256(key, label)`: every key this node derives from another is one.
pub(crate) fn tag(key: &[u8; 32], label: impl AsRef<[u8]>) -> [u8; 32] {
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), label.as_ref());
    let mut out = [0; 32];
    out.copy_from_slice(&tag.as_ref()[..32]);
    out
}

/// A backup's file name: the index it is at, the app's group, the snapshot.
#[must_use]
pub fn file_name(index: u64, group: u64, snapshot_id: &str) -> String {
    format!("{index:020}.{group}.{snapshot_id}.snap")
}

/// One row of an app's state: `(key, value)`.
pub type Row = (Vec<u8>, Vec<u8>);

/// The newest snapshot of `app_id` under `dir`: its applied index, and the
/// rows of the app's state (`(key, value)`, the log's own layout).
pub fn latest(
    dir: &Path,
    cluster_key: &[u8; 32],
    app_id: &str,
) -> eyre::Result<Option<(u64, Vec<Row>)>> {
    let path = dir.join(app_id);
    if !path.exists() {
        return Ok(None);
    }
    let mut files: Vec<_> = std::fs::read_dir(&path)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "snap"))
        .collect();
    files.sort();
    let Some(file) = files.pop() else {
        return Ok(None);
    };
    let name = file
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| eyre::eyre!("bad backup name {}", file.display()))?;
    let mut parts = name.splitn(3, '.');
    let (Some(index), Some(group), Some(id)) = (parts.next(), parts.next(), parts.next()) else {
        eyre::bail!("bad backup name {name}");
    };
    let mut rows = Vec::new();
    calimero_log::read_snapshot_file(cluster_key, group.parse()?, id, &file, |k, v| {
        rows.push((k.to_vec(), v.to_vec()));
        Ok(())
    })?;
    Ok(Some((index.parse()?, rows)))
}

/// The backup format this release writes: a snapshot file and its sealed
/// [`Manifest`]. Format 0 is a snapshot file alone, as releases before
/// manifests wrote it; this release reads both.
pub const FORMAT: u32 = 1;

/// What a manifest's file name ends with, beside its snapshot's.
pub const MANIFEST_SUFFIX: &str = ".manifest";

/// A backup file name's longest form: `{index:020}.{group}.{snapshot}.snap`.
pub const MAX_NAME_LEN: usize = 160;

/// A manifest is never larger: a few short fields, sealed.
pub const MAX_MANIFEST_BYTES: u64 = 16 * 1024;

const MANIFEST_MAGIC: &[u8] = b"calimero.backup-manifest.v1\n";
const MANIFEST_LABEL: &str = "calimero.backup.manifest.v1";

/// What a backup records about itself (sealed beside the snapshot file as
/// `{name}.manifest`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// The backup format ([`FORMAT`]); a release reads the formats up to its own.
    pub format: u32,
    /// The `merod` version that wrote it: a node of an older version does not
    /// restore it ([`Found::check_release`]).
    pub merod: String,
    /// The mero-tee release the writing node attested.
    pub release: String,
    pub app: String,
    pub group: u64,
    /// The app log's applied index the snapshot is at.
    pub index: u64,
    pub snapshot_id: String,
    /// SHA-256 of the snapshot file, hex.
    pub sha256: String,
    /// When it was written, Unix seconds.
    pub written_at: u64,
    /// The node that wrote it.
    pub node: u64,
}

/// What any manifest says first, whatever its format: read before the rest,
/// so a newer format is named as such rather than refused as malformed.
#[derive(Deserialize)]
struct ManifestHead {
    format: u32,
    #[serde(default)]
    merod: String,
}

fn manifest_sealer(cluster_key: &[u8; 32]) -> Sealer {
    Sealer::new(&tag(cluster_key, MANIFEST_LABEL))
}

fn manifest_aad(app: &str, name: &str) -> Vec<u8> {
    let mut aad = MANIFEST_MAGIC.to_vec();
    aad.extend_from_slice(&u32::try_from(app.len()).unwrap_or(u32::MAX).to_be_bytes());
    aad.extend_from_slice(app.as_bytes());
    aad.extend_from_slice(name.as_bytes());
    aad
}

/// `manifest` sealed for the snapshot file `name` of its app.
pub fn seal_manifest(
    cluster_key: &[u8; 32],
    name: &str,
    manifest: &Manifest,
) -> eyre::Result<Vec<u8>> {
    let plain = serde_json::to_vec(manifest)?;
    let mut out = MANIFEST_MAGIC.to_vec();
    out.extend(manifest_sealer(cluster_key).seal(&manifest_aad(&manifest.app, name), &plain)?);
    Ok(out)
}

/// Why a backup is not restored.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("no backup of this app named {0} in the backup directory")]
    NotFound(String),
    #[error("{0}")]
    Bad(String),
    #[error(
        "this backup was written in backup format {format} (by merod {merod}), which this \
         release does not read (formats 0 to {FORMAT}); restore it on that release or a newer one"
    )]
    UnknownFormat { format: u32, merod: String },
    #[error(
        "this backup was written by merod {backup}, newer than this node's {own}: a release \
         does not restore what a newer one wrote; restore it on {backup} or newer"
    )]
    NewerRelease { backup: String, own: String },
}

impl BackupError {
    /// The API's error code for it.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "BACKUP_NOT_FOUND",
            Self::Bad(_) => "BAD_BACKUP",
            Self::UnknownFormat { .. } => "UNKNOWN_BACKUP_FORMAT",
            Self::NewerRelease { .. } => "BACKUP_FROM_NEWER_RELEASE",
        }
    }
}

fn bad(e: impl std::fmt::Display) -> BackupError {
    BackupError::Bad(e.to_string())
}

/// A backup file found, checked against its manifest.
#[derive(Clone, Debug)]
pub struct Found {
    pub path: PathBuf,
    /// The snapshot file's name in the app's backup directory.
    pub name: String,
    pub index: u64,
    /// The group the snapshot's frames and rows are sealed for.
    pub group: u64,
    pub snapshot_id: String,
    /// SHA-256 of the file, hex.
    pub sha256: String,
    /// `None`: format 0, a backup written before manifests.
    pub manifest: Option<Manifest>,
}

impl Found {
    /// The backup format it was written in.
    #[must_use]
    pub fn format(&self) -> u32 {
        self.manifest.as_ref().map_or(0, |m| m.format)
    }

    /// Refuse a backup a newer `merod` than `own` wrote: its state may hold
    /// what this release does not know, and would drop.
    ///
    /// # Errors
    /// [`BackupError::NewerRelease`].
    pub fn check_release(&self, own: &str) -> Result<(), BackupError> {
        let Some(m) = &self.manifest else {
            return Ok(());
        };
        let newer = match (
            semver::Version::parse(&m.merod),
            semver::Version::parse(own),
        ) {
            (Ok(backup), Ok(own)) => backup > own,
            _ => m.merod != own,
        };
        if newer {
            return Err(BackupError::NewerRelease {
                backup: m.merod.clone(),
                own: own.to_owned(),
            });
        }
        Ok(())
    }
}

/// Whether `name` can be a backup file's name: no path, the snapshot form.
fn valid_name(name: &str) -> bool {
    name.len() <= MAX_NAME_LEN
        && name.ends_with(".snap")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
        && !name.starts_with('.')
}

/// SHA-256 of a file, hex, read in pieces.
pub fn file_sha256(path: &Path) -> eyre::Result<String> {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0_u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Ok(hex::encode(ctx.finish()))
}

/// Find the backup `name` of `app_id` under `dir` (the newest when `None`),
/// and check it against its manifest: the manifest opens under this cluster's
/// key for this app and file name, its format is one this release reads, and
/// it names the file's index, group, snapshot and SHA-256. A file without a
/// manifest is format 0, judged by its name alone (the caller checks that its
/// group is the app's).
///
/// # Errors
/// No such backup, a manifest that does not open or match, or an unknown format.
pub fn find(
    dir: &Path,
    cluster_key: &[u8; 32],
    app_id: &str,
    name: Option<&str>,
) -> Result<Found, BackupError> {
    let app_dir = dir.join(app_id);
    let name = match name {
        Some(n) if !valid_name(n) => {
            return Err(BackupError::Bad(format!("{n:?} is not a backup file name")))
        }
        Some(n) => n.to_owned(),
        None => {
            let mut names: Vec<String> = std::fs::read_dir(&app_dir)
                .map_err(|_| BackupError::NotFound("(any)".into()))?
                .filter_map(Result::ok)
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| valid_name(n))
                .collect();
            names.sort();
            names
                .pop()
                .ok_or_else(|| BackupError::NotFound("(any)".into()))?
        }
    };
    let path = app_dir.join(&name);
    if !path.is_file() {
        return Err(BackupError::NotFound(name));
    }
    let stem = name.trim_end_matches(".snap");
    let mut parts = stem.splitn(3, '.');
    let (Some(index), Some(group), Some(snapshot_id)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(BackupError::Bad(format!(
            "{name:?} is not a backup file name"
        )));
    };
    let index: u64 = index.parse().map_err(bad)?;
    let group: u64 = group.parse().map_err(bad)?;
    let sha256 = file_sha256(&path).map_err(bad)?;
    let manifest_path = app_dir.join(format!("{name}{MANIFEST_SUFFIX}"));
    if std::fs::metadata(&manifest_path).is_ok_and(|m| m.len() > MAX_MANIFEST_BYTES) {
        return Err(BackupError::Bad(
            "the backup's manifest is too large".into(),
        ));
    }
    let manifest = match std::fs::read(&manifest_path) {
        Ok(bytes) => Some(open_manifest(cluster_key, app_id, &name, &bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(bad(e)),
    };
    if let Some(m) = &manifest {
        let matches = m.app == app_id
            && m.index == index
            && m.group == group
            && m.snapshot_id == snapshot_id
            && m.sha256 == sha256;
        if !matches {
            return Err(BackupError::Bad(
                "the backup file is not the one its manifest names".into(),
            ));
        }
    }
    Ok(Found {
        path,
        name,
        index,
        group,
        snapshot_id: snapshot_id.to_owned(),
        sha256,
        manifest,
    })
}

/// Open a manifest sealed for `app_id`'s backup file `name`.
///
/// # Errors
/// It does not open under this cluster's key for that app and name, or names a
/// format this release does not read.
pub fn open_manifest(
    cluster_key: &[u8; 32],
    app_id: &str,
    name: &str,
    bytes: &[u8],
) -> Result<Manifest, BackupError> {
    let sealed = bytes
        .strip_prefix(MANIFEST_MAGIC)
        .ok_or_else(|| BackupError::Bad("not a backup manifest".into()))?;
    let plain = manifest_sealer(cluster_key)
        .open(&manifest_aad(app_id, name), sealed)
        .map_err(|_| {
            BackupError::Bad(
                "the backup's manifest does not open: another cluster wrote it, or it belongs \
                 to another app or file"
                    .into(),
            )
        })?;
    let head: ManifestHead = serde_json::from_slice(&plain).map_err(bad)?;
    if head.format > FORMAT {
        return Err(BackupError::UnknownFormat {
            format: head.format,
            merod: head.merod,
        });
    }
    serde_json::from_slice(&plain).map_err(bad)
}

/// Copy the snapshot file `snapshot` of `manifest.app` into `dir` as a backup,
/// with its manifest beside it. Answers whether it wrote one (`false`: that
/// backup is there already).
pub fn write(
    dir: &Path,
    cluster_key: &[u8; 32],
    snapshot: &Path,
    manifest: &Manifest,
) -> eyre::Result<bool> {
    let path = dir.join(&manifest.app);
    std::fs::create_dir_all(&path)?;
    let name = file_name(manifest.index, manifest.group, &manifest.snapshot_id);
    let out = path.join(&name);
    if out.exists() {
        return Ok(false);
    }
    // The manifest first, the file it names last: a backup is listed only
    // once both are there.
    let tmp = path.join(format!(".{name}.tmp"));
    let _bytes = std::fs::copy(snapshot, &tmp)?;
    let mut manifest = manifest.clone();
    manifest.sha256 = file_sha256(&tmp)?;
    std::fs::write(
        path.join(format!("{name}{MANIFEST_SUFFIX}")),
        seal_manifest(cluster_key, &name, &manifest)?,
    )?;
    std::fs::rename(&tmp, &out)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(app: &str, name: &str) -> Manifest {
        let stem = name.trim_end_matches(".snap");
        let parts: Vec<&str> = stem.splitn(3, '.').collect();
        Manifest {
            format: FORMAT,
            merod: "1.0.0".into(),
            release: "1.0.0".into(),
            app: app.into(),
            group: parts[1].parse().unwrap(),
            index: parts[0].parse().unwrap(),
            snapshot_id: parts[2].into(),
            sha256: String::new(),
            written_at: 0,
            node: 1,
        }
    }

    /// A backup directory holding `app`'s snapshot `name` with `bytes`,
    /// written as `write` writes it.
    fn backed_up(key: &[u8; 32], app: &str, name: &str, bytes: &[u8]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let snap = dir.path().join("source.snap");
        std::fs::write(&snap, bytes).unwrap();
        assert!(write(dir.path(), key, &snap, &manifest(app, name)).unwrap());
        assert!(!write(dir.path(), key, &snap, &manifest(app, name)).unwrap());
        dir
    }

    #[test]
    fn a_backup_is_found_with_its_manifest_and_only_under_its_own_name() {
        let key = [7; 32];
        let name = file_name(42, 3, "42-1-abc");
        let dir = backed_up(&key, "app", &name, b"frames");
        let found = find(dir.path(), &key, "app", None).unwrap();
        assert_eq!((found.index, found.group), (42, 3));
        assert_eq!(found.snapshot_id, "42-1-abc");
        assert_eq!(found.format(), FORMAT);
        assert_eq!(
            find(dir.path(), &key, "app", Some(&name)).unwrap().name,
            name
        );

        // Another cluster's key opens no manifest.
        let e = find(dir.path(), &[8; 32], "app", None).unwrap_err();
        assert_eq!(e.code(), "BAD_BACKUP", "{e}");
        // Moved under another app, it is refused.
        std::fs::rename(dir.path().join("app"), dir.path().join("other")).unwrap();
        let e = find(dir.path(), &key, "other", None).unwrap_err();
        assert_eq!(e.code(), "BAD_BACKUP", "{e}");
        std::fs::rename(dir.path().join("other"), dir.path().join("app")).unwrap();
        // Another file under its name is refused.
        std::fs::write(dir.path().join("app").join(&name), b"swapped").unwrap();
        let e = find(dir.path(), &key, "app", None).unwrap_err();
        assert_eq!(e.code(), "BAD_BACKUP", "{e}");
        // Names are file names only.
        for bad_name in ["../app/x.snap", "x", ".hidden.snap"] {
            let e = find(dir.path(), &key, "app", Some(bad_name)).unwrap_err();
            assert_eq!(e.code(), "BAD_BACKUP", "{bad_name}: {e}");
        }
        let e = find(
            dir.path(),
            &key,
            "app",
            Some("00000000000000000001.3.x.snap"),
        )
        .unwrap_err();
        assert_eq!(e.code(), "BACKUP_NOT_FOUND", "{e}");
    }

    #[test]
    fn a_backup_without_a_manifest_is_format_0() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app")).unwrap();
        let name = file_name(5, 2, "5-1-abc");
        std::fs::write(dir.path().join("app").join(&name), b"frames").unwrap();
        let found = find(dir.path(), &[7; 32], "app", None).unwrap();
        assert_eq!(found.format(), 0);
        assert!(found.manifest.is_none());
        found.check_release("0.0.1").unwrap();
    }

    #[test]
    fn a_newer_release_or_format_is_refused_clearly() {
        let key = [7; 32];
        let name = file_name(1, 2, "1-1-abc");
        let dir = backed_up(&key, "app", &name, b"frames");
        let found = find(dir.path(), &key, "app", None).unwrap();
        found.check_release("1.0.0").unwrap();
        found.check_release("1.2.0").unwrap();
        let e = found.check_release("1.0.0-rc.15").unwrap_err();
        assert_eq!(e.code(), "BACKUP_FROM_NEWER_RELEASE", "{e}");
        assert!(e.to_string().contains("1.0.0"), "{e}");

        let mut future = manifest("app", &name);
        future.format = FORMAT + 1;
        future.merod = "9.0.0".into();
        let sealed = seal_manifest(&key, &name, &future).unwrap();
        let e = open_manifest(&key, "app", &name, &sealed).unwrap_err();
        assert_eq!(e.code(), "UNKNOWN_BACKUP_FORMAT", "{e}");
        assert!(e.to_string().contains("9.0.0"), "{e}");
    }

    #[test]
    fn the_manifest_key_matches_its_known_answer() {
        // HMAC-SHA256([7; 32], "calimero.backup.manifest.v1"), as every node of
        // every release derives it.
        assert_eq!(
            hex::encode(tag(&[7; 32], MANIFEST_LABEL)),
            "1be60017d3afba7725d99b941761ca8a2797c39a33b0edb3c2ed51613d53570d"
        );
        let m = manifest("app", &file_name(1, 2, "1-1-abc"));
        let mut keys: Vec<String> = serde_json::to_value(&m)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "app",
                "format",
                "group",
                "index",
                "merod",
                "node",
                "release",
                "sha256",
                "snapshotId",
                "writtenAt"
            ]
        );
    }

    #[test]
    fn the_file_key_matches_its_known_answer() {
        assert_eq!(
            hex::encode(file_key(&[7; 32], "app")),
            "fd6f5681b66993559e8d5027662473049b1a25fe667a1383454003d7db41fdf6"
        );
    }

    #[test]
    fn the_cluster_identity_matches_its_known_answer() {
        // HMAC-SHA256([1; 32], "calimero.cluster-identity.v1") as the Ed25519 seed.
        let key = cluster_identity_key(&[1; 32]);
        assert_eq!(
            hex::encode(key.to_bytes()),
            "e0dbdecf4f52790ed642083a9a60385823234a1334b17efb432568cab988a4db"
        );
        assert_eq!(
            hex::encode(key.verifying_key().to_bytes()),
            "c0bfb91fd240b8b36345da220f2a4040028c563d3499d01af91919e90fe1990a"
        );
        assert_ne!(
            key.verifying_key(),
            cluster_identity_key(&[2; 32]).verifying_key()
        );
    }
}
