//! A component app's blobs (R8): the host's record of each blob the app holds, kept by
//! references in state (host-tracked retention), and the download grants runs hand out.
//!
//! The bytes live in the cluster's blob store, as a wasmer app's do (`Node::store_blob`,
//! spread to a majority, fetched from a peer when missing). What differs is how long an app
//! holds one: a wasmer app marks a blob for good until its owner deletes it; a component's
//! blob is held while a stored value refers to it (a `BlobRef`, CBOR tag
//! `calimero_app_host::BLOB_REF_TAG`), plus [`BLOB_IDLE_MS`] after its upload or after its
//! last reference went, so a client can upload and then attach. Every replica counts the
//! references from the diffs it applies and the values they replace, and forgets an idle
//! blob against entry timestamps (never a node's clock), so all replicas agree. The leader
//! then collects the bytes (`Node::collect_blob`, unless another app holds the same
//! content) and logs that it did (`ComponentCmd::BlobsCollected`).
//!
//! The records are host-owned (D39): hidden rows of the group (`rows::CBLOB`) and, in
//! memory, an ordered map shared with runs as their view of the app's blobs.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use calimero_app_host::{BlobHost, BlobInfo};
use calimero_log::{GroupState, StateTx};
use ring::hmac;
use serde::{Deserialize, Serialize};

use crate::app::rows;

/// How long a blob nothing refers to is held: after its upload, or after the last value
/// referring to it was overwritten or deleted.
pub const BLOB_IDLE_MS: u64 = 60 * 60 * 1_000;
/// Idle blobs one entry forgets at most; the rest go with later entries.
pub const BLOB_EXPIRY_STEP: usize = 256;
/// Blobs one component app holds at most; an upload past it is refused.
pub const MAX_COMPONENT_BLOBS: usize = 100_000;
/// Forgotten blobs whose bytes wait for the leader to collect them; past it the oldest are
/// dropped from the queue (their bytes stay in the store until the owner's delete).
pub const MAX_COLLECT_QUEUE: usize = 4_096;
/// Bytes of a blob's content type.
pub const MAX_BLOB_MIME_BYTES: usize = 127;
/// Bytes of a grant a download presents (`<expires_ms>.<base64url HMAC>` is 64 at most);
/// a longer one is refused before it is parsed.
pub const MAX_GRANT_BYTES: usize = 128;
/// Blobs one `BlobsCollected` entry names at most.
pub const COLLECT_BATCH: usize = 64;

/// What the host records of one of a component app's blobs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobRecord {
    pub size: u64,
    pub mime: String,
    /// Stored values referring to it.
    pub refs: u64,
    /// When it became unreferenced (its upload, or its last reference going); meaningless
    /// while `refs > 0`.
    pub idle_since_ms: u64,
}

/// A component app's blobs, by SHA-256.
pub type Blobs = Arc<BTreeMap<[u8; 32], BlobRecord>>;

/// The app's blobs on a replica: the records, and the unreferenced ones by the time they
/// became so (rebuilt from the records on load).
#[derive(Clone, Debug, Default)]
pub struct BlobTable {
    pub records: Blobs,
    idle: BTreeSet<(u64, [u8; 32])>,
}

impl BlobTable {
    /// Read the records from the group's rows (a restart, a snapshot install).
    pub fn load(&mut self, state: &GroupState) -> eyre::Result<()> {
        let mut records = BTreeMap::new();
        state.scan_hidden(rows::CBLOB, |k, v| {
            if let (Ok(id), Ok(r)) = (
                <[u8; 32]>::try_from(k),
                serde_json::from_slice::<BlobRecord>(v),
            ) {
                let _prev = records.insert(id, r);
            }
            Ok(true)
        })?;
        self.idle = records
            .iter()
            .filter(|(_, r)| r.refs == 0)
            .map(|(id, r)| (r.idle_since_ms, *id))
            .collect();
        self.records = Arc::new(records);
        Ok(())
    }

    /// Record an upload at `ts_ms`. A blob the app already holds keeps its references; an
    /// idle one is held [`BLOB_IDLE_MS`] from now. Refused (`false`) past
    /// [`MAX_COMPONENT_BLOBS`].
    pub fn upload(
        &mut self,
        tx: &mut StateTx<'_>,
        id: [u8; 32],
        size: u64,
        mime: &str,
        ts_ms: u64,
    ) -> bool {
        let prev = self.records.get(&id).cloned();
        if prev.is_none() && self.records.len() >= MAX_COMPONENT_BLOBS {
            return false;
        }
        let refs = prev.as_ref().map_or(0, |p| p.refs);
        if let Some(p) = &prev {
            if p.refs == 0 {
                let _was = self.idle.remove(&(p.idle_since_ms, id));
            }
        }
        let record = BlobRecord {
            size,
            mime: mime.to_owned(),
            refs,
            idle_since_ms: ts_ms,
        };
        if refs == 0 {
            let _new = self.idle.insert((ts_ms, id));
        }
        self.put(tx, id, record);
        true
    }

    /// Forget a blob (its owner deleted it); whether the app held it.
    pub fn forget(&mut self, tx: &mut StateTx<'_>, id: &[u8; 32]) -> bool {
        let Some(prev) = Arc::make_mut(&mut self.records).remove(id) else {
            return false;
        };
        let _was = self.idle.remove(&(prev.idle_since_ms, *id));
        tx.delete_hidden(rows::CBLOB, id);
        true
    }

    /// Change the reference counts by `deltas` (from one applied diff). References to a blob
    /// the app does not hold change nothing; a count reaching zero starts the idle time.
    pub fn count(&mut self, tx: &mut StateTx<'_>, deltas: &BTreeMap<[u8; 32], i64>, ts_ms: u64) {
        for (id, delta) in deltas {
            let Some(mut r) = self.records.get(id).cloned() else {
                continue;
            };
            let before = r.refs;
            r.refs = if *delta >= 0 {
                before.saturating_add(delta.unsigned_abs())
            } else {
                before.saturating_sub(delta.unsigned_abs())
            };
            if r.refs == before {
                continue;
            }
            if before == 0 {
                let _was = self.idle.remove(&(r.idle_since_ms, *id));
            }
            if r.refs == 0 {
                r.idle_since_ms = ts_ms;
                let _new = self.idle.insert((ts_ms, *id));
            }
            self.put(tx, *id, r);
        }
    }

    /// Forget the blobs idle for [`BLOB_IDLE_MS`] at `now_ms`, at most [`BLOB_EXPIRY_STEP`];
    /// returns them.
    pub fn expire(&mut self, tx: &mut StateTx<'_>, now_ms: u64) -> Vec<[u8; 32]> {
        let due: Vec<(u64, [u8; 32])> = self
            .idle
            .iter()
            .take_while(|(since, _)| since.saturating_add(BLOB_IDLE_MS) <= now_ms)
            .take(BLOB_EXPIRY_STEP)
            .copied()
            .collect();
        due.into_iter()
            .filter_map(|(_, id)| self.forget(tx, &id).then_some(id))
            .collect()
    }

    fn put(&mut self, tx: &mut StateTx<'_>, id: [u8; 32], record: BlobRecord) {
        match serde_json::to_vec(&record) {
            Ok(v) => tx.put_hidden(rows::CBLOB, &id, &v),
            Err(e) => tx.fail(e),
        }
        let _prev = Arc::make_mut(&mut self.records).insert(id, record);
    }
}

/// The reference-count changes of replacing `old` with `new` in one stored value.
pub fn ref_deltas(deltas: &mut BTreeMap<[u8; 32], i64>, old: Option<&[u8]>, new: Option<&[u8]>) {
    for id in old.map(calimero_app_host::blob_refs).unwrap_or_default() {
        *deltas.entry(id).or_default() -= 1;
    }
    for id in new.map(calimero_app_host::blob_refs).unwrap_or_default() {
        *deltas.entry(id).or_default() += 1;
    }
}

/// The key download grants are MACed with: HMAC of K_c over a fixed label, so every node of
/// the cluster checks a grant any of them handed out.
#[must_use]
pub fn grant_key(cluster_key: &[u8; 32]) -> [u8; 32] {
    let tag = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, cluster_key),
        b"calimero.blob-grants.v1",
    );
    let mut out = [0; 32];
    out.copy_from_slice(tag.as_ref());
    out
}

fn grant_message(app_id: &str, id: &[u8; 32], expires_ms: u64) -> Vec<u8> {
    format!("{app_id}|{}|{expires_ms}", hex::encode(id)).into_bytes()
}

/// A grant to download blob `id` of `app_id` until `expires_ms` (app time):
/// `<expires_ms>.<base64url HMAC>`.
#[must_use]
pub fn grant_token(key: &[u8; 32], app_id: &str, id: &[u8; 32], expires_ms: u64) -> String {
    let mac = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, key),
        &grant_message(app_id, id, expires_ms),
    );
    format!("{expires_ms}.{}", calimero_identity::b64url(mac.as_ref()))
}

/// Whether `token` grants blob `id` of `app_id` at `now_ms` (app time).
#[must_use]
pub fn grant_valid(key: &[u8; 32], app_id: &str, id: &[u8; 32], token: &str, now_ms: u64) -> bool {
    if token.len() > MAX_GRANT_BYTES {
        return false;
    }
    let Some((expires, mac)) = token.split_once('.') else {
        return false;
    };
    let (Ok(expires_ms), Ok(mac)) = (expires.parse::<u64>(), calimero_identity::from_b64url(mac))
    else {
        return false;
    };
    if now_ms > expires_ms {
        return false;
    }
    hmac::verify(
        &hmac::Key::new(hmac::HMAC_SHA256, key),
        &grant_message(app_id, id, expires_ms),
        &mac,
    )
    .is_ok()
}

/// What a run sees of the app's blobs: the replica's records at the state it reads.
pub struct RunBlobs {
    pub app_id: String,
    pub records: Blobs,
    pub key: [u8; 32],
}

impl BlobHost for RunBlobs {
    fn info(&self, id: &[u8; 32]) -> Option<BlobInfo> {
        self.records.get(id).map(|r| BlobInfo {
            size: r.size,
            sha256: id.to_vec(),
            mime: r.mime.clone(),
        })
    }

    fn grant(&self, id: &[u8; 32], expires_ms: u64) -> String {
        grant_token(&self.key, &self.app_id, id, expires_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_holds_for_its_blob_and_app_until_it_expires() {
        let key = grant_key(&[1; 32]);
        let t = grant_token(&key, "app", &[2; 32], 1_000);
        assert!(grant_valid(&key, "app", &[2; 32], &t, 1_000));
        assert!(!grant_valid(&key, "app", &[2; 32], &t, 1_001), "expired");
        assert!(!grant_valid(&key, "other", &[2; 32], &t, 0), "another app");
        assert!(!grant_valid(&key, "app", &[3; 32], &t, 0), "another blob");
        let forged = t.replacen("1000", "9000", 1);
        assert!(
            !grant_valid(&key, "app", &[2; 32], &forged, 0),
            "moved expiry"
        );
        assert!(
            !grant_valid(&grant_key(&[9; 32]), "app", &[2; 32], &t, 0),
            "another cluster"
        );
    }

    #[test]
    fn a_value_replaced_moves_its_references() {
        let tagged = |id: u8| {
            let mut out = Vec::new();
            ciborium::into_writer(
                &ciborium::Value::Tag(
                    calimero_app_host::BLOB_REF_TAG,
                    Box::new(ciborium::Value::Bytes(vec![id; 32])),
                ),
                &mut out,
            )
            .unwrap();
            out
        };
        let mut d = BTreeMap::new();
        ref_deltas(&mut d, Some(&tagged(1)), Some(&tagged(2)));
        ref_deltas(&mut d, None, Some(&tagged(2)));
        assert_eq!(d.get(&[1; 32]), Some(&-1));
        assert_eq!(d.get(&[2; 32]), Some(&2));
    }
}
