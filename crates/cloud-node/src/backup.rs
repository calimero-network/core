//! Encrypted app snapshots in object storage (a directory stands in for a bucket).

use std::path::Path;

use ring::hmac;

/// The key an app's snapshots are sealed with: HMAC(K_r, "calimero.snapshot.v1|" app).
#[must_use]
pub fn snapshot_key(storage_key: &[u8; 32], app_id: &str) -> [u8; 32] {
    let tag = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, storage_key),
        format!("calimero.snapshot.v1|{app_id}").as_bytes(),
    );
    let mut out = [0; 32];
    out.copy_from_slice(&tag.as_ref()[..32]);
    out
}

/// The newest snapshot of `app_id` under `dir`: its applied index and state.
pub fn latest(
    dir: &Path,
    storage_key: &[u8; 32],
    app_id: &str,
) -> eyre::Result<Option<(u64, Vec<u8>)>> {
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
    let index: u64 = file
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let sealer = calimero_log::store::Sealer::new(&snapshot_key(storage_key, app_id));
    let data = sealer.open(app_id.as_bytes(), &std::fs::read(&file)?)?;
    Ok(Some((index, data)))
}
