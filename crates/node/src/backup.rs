//! Encrypted app snapshots in object storage (a directory stands in for a
//! bucket), and the keys the cluster derives per app from its cluster key K_c
//! (`crate::cluster_key`).

use std::path::Path;

use ring::hmac;

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

/// The cluster's identity: an Ed25519 key only a node holding the cluster key can make, so a
/// signature by it proves a node of this cluster signed. Its public half names the cluster to
/// the job nodes that take its work.
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

#[cfg(test)]
mod tests {
    use super::*;

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
