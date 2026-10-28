//! Encrypted app snapshots in object storage (a directory stands in for a
//! bucket), and the keys the cluster derives per app from K_r.

use std::path::Path;

use ring::hmac;

/// The app's TEE key: what values sealed to the app's TEE (`TeeSecret`, an
/// `#[app::tee]` run's own state) open with. The cluster is the app's TEE, so
/// every node derives the same key from K_r, and nothing outside it can.
#[must_use]
pub fn tee_key(storage_key: &[u8; 32], app_id: &str) -> [u8; 32] {
    tag(storage_key, &format!("calimero.tee.v1|{app_id}"))
}

/// The key that stands for `account`'s devices in `app`: a member's share is
/// sealed to it, not to each device; the member reads through the node,
/// which opens it only in that member's own runs.
#[must_use]
pub fn member_key(storage_key: &[u8; 32], app_id: &str, account: &str) -> [u8; 32] {
    tag(
        storage_key,
        &format!("calimero.member.v1|{app_id}|{account}"),
    )
}

fn tag(storage_key: &[u8; 32], label: &str) -> [u8; 32] {
    let tag = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, storage_key),
        label.as_bytes(),
    );
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
    storage_key: &[u8; 32],
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
    calimero_log::read_snapshot_file(storage_key, group.parse()?, id, &file, |k, v| {
        rows.push((k.to_vec(), v.to_vec()));
        Ok(())
    })?;
    Ok(Some((index.parse()?, rows)))
}
