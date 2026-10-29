//! What a stopped node's data directory holds (`merod inspect`),
//! read offline. Without the storage key: the raw layout (sizes
//! are sealed sizes). With it: each group's metadata (vote, applied index,
//! membership) too. The store is opened for writing, so stop the node first.

use std::path::Path;

use serde_json::Value;

pub fn inspect(data_dir: &Path, storage_key: Option<[u8; 32]>) -> eyre::Result<Value> {
    let store = calimero_log::store::store_path(data_dir);
    if !store.exists() {
        eyre::bail!("{} holds no node store", data_dir.display());
    }
    // The store is sealed under the cluster key, which K_r unwraps; a store
    // from before the cluster key is sealed under K_r itself.
    let key = match storage_key {
        Some(k_r) => Some(crate::cluster_key::load(data_dir, &k_r)?.unwrap_or(k_r)),
        None => None,
    };
    let db = calimero_log::store::Db::open(data_dir, &key.unwrap_or([0; 32]))?;
    let summary = db.summary()?;
    let mut out = serde_json::to_value(&summary)?;
    if key.is_some() {
        for (group, g) in &summary.groups {
            for name in &g.meta {
                let value: Option<Value> = db
                    .get_meta(*group, name)
                    .map_err(|e| eyre::eyre!("group {group} {name}: {e} (wrong key?)"))?;
                out["groups"][group.to_string()]["values"][name] = value.unwrap_or_default();
            }
        }
    }
    Ok(out)
}
