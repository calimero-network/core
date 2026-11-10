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

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use calimero_app_host::{BlobHost, BlobInfo};
use calimero_log::{Bytes, GroupState, StateTx};
use ring::hmac;
use serde::{Deserialize, Serialize};

use crate::app::rows;
use crate::component::FileMark;

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
/// Bytes of a grant a download presents (`<expires_ms>.<epoch>.<base64url HMAC>` is 75 at most);
/// a longer one is refused before it is parsed.
pub const MAX_GRANT_BYTES: usize = 128;
/// Blobs one `BlobsCollected` entry names at most.
pub const COLLECT_BATCH: usize = 64;
/// Key of the app's epoch floor in `rows::CBLOB`; a blob id is 32 bytes, so it never collides.
pub(crate) const FLOOR_KEY: &[u8] = b"floor";

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
    /// Set for a sealed file: what opening it needs beyond the ciphertext.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<FileSeal>,
    /// Set for a plain blob made by publishing a compute output: its size is charged to the
    /// file quota, which collecting it releases.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub charged: bool,
    /// How many times the owner revoked the grants of a published blob: a grant names the
    /// count it was made under, and one made under another no longer holds.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub epoch: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl BlobRecord {
    /// The record of the sealed file `mark` names, unreferenced and idle from `ts_ms`, and its
    /// id; none when the mark's id is not the hash of its segment list.
    #[must_use]
    pub fn sealed(mark: &FileMark, ts_ms: u64) -> Option<([u8; 32], Self)> {
        let id = <[u8; 32]>::try_from(hex::decode(&mark.id).ok()?).ok()?;
        mark.segment_hashes()?;
        let record = Self {
            size: mark.size,
            mime: String::new(),
            refs: 0,
            idle_since_ms: ts_ms,
            file: Some(FileSeal {
                sha256: mark.sha256.clone(),
                wrapped_key: mark.wrapped_key.clone(),
                segments: mark.segments.clone(),
                output: false,
                uploader: None,
            }),
            charged: false,
            epoch: 0,
        };
        Some((id, record))
    }
}

/// What the record of a sealed file adds to a blob's: the plaintext hash and the data key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSeal {
    /// SHA-256 of the plaintext.
    pub sha256: Bytes,
    /// The data key, wrapped under the app's file key.
    pub wrapped_key: Bytes,
    /// Hex SHA-256 of each segment of the ciphertext, in order.
    pub segments: Vec<String>,
    /// Set when the file is an output of a done compute run: the only kind of file that
    /// can be published.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub output: bool,
    /// The account that uploaded the file, as the authenticated upload route saw it; none for a
    /// file a call or a run made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploader: Option<String>,
}

/// A blob forgotten by retention: its id, the quota it held, and the segments of a sealed file.
pub type Expired = ([u8; 32], Option<u64>);

/// A component app's blobs, by SHA-256.
pub type Blobs = Arc<BTreeMap<[u8; 32], BlobRecord>>;

/// The app's blobs on a replica: the records, and the unreferenced ones by the time they
/// became so (rebuilt from the records on load).
#[derive(Clone, Debug, Default)]
pub struct BlobTable {
    pub records: Blobs,
    idle: BTreeSet<(u64, [u8; 32])>,
    /// Above the epoch of every grantable blob forgotten so far: a record made again starts
    /// here, so a grant of a forgotten blob never holds for its successor.
    floor: u32,
    /// The segments the sealed files list, by hash, to the file: rebuilt from the records on load.
    segments: HashMap<String, [u8; 32]>,
}

impl BlobTable {
    /// Whether a sealed file of the app lists segment `hash`.
    #[must_use]
    pub fn holds_segment(&self, hash: &str) -> bool {
        self.segments.contains_key(hash)
    }

    fn index_segments(&mut self, id: &[u8; 32], record: &BlobRecord) {
        for hash in record.file.iter().flat_map(|f| &f.segments) {
            let _prev = self.segments.insert(hash.clone(), *id);
        }
    }

    fn unindex_segments(&mut self, id: &[u8; 32]) {
        let Some(file) = self.records.get(id).and_then(|r| r.file.as_ref()) else {
            return;
        };
        for hash in &file.segments {
            if self.segments.get(hash) == Some(id) {
                let _gone = self.segments.remove(hash);
            }
        }
    }

    /// Read the records from the group's rows (a restart, a snapshot install).
    pub fn load(&mut self, state: &GroupState) -> eyre::Result<()> {
        let mut records = BTreeMap::new();
        let mut floor = 0;
        state.scan_hidden(rows::CBLOB, |k, v| {
            if k == FLOOR_KEY {
                floor = serde_json::from_slice(v)
                    .map_err(|e| eyre::eyre!("the blob epoch floor is unreadable: {e}"))?;
            }
            if let Ok(id) = <[u8; 32]>::try_from(k) {
                let r = serde_json::from_slice::<BlobRecord>(v)
                    .map_err(|e| eyre::eyre!("a blob record is unreadable: {e}"))?;
                let _prev = records.insert(id, r);
            }
            Ok(true)
        })?;
        self.idle = records
            .iter()
            .filter(|(_, r)| r.refs == 0)
            .map(|(id, r)| (r.idle_since_ms, *id))
            .collect();
        self.segments = (records.iter())
            .flat_map(|(id, r)| {
                r.file
                    .iter()
                    .flat_map(|f| &f.segments)
                    .map(|h| (h.clone(), *id))
            })
            .collect();
        self.records = Arc::new(records);
        self.floor = floor;
        Ok(())
    }

    /// Record an upload at `ts_ms`. A blob the app already holds keeps its references; an
    /// idle one is held [`BLOB_IDLE_MS`] from now. Refused (`false`) past
    /// [`MAX_COMPONENT_BLOBS`] or when `id` is a sealed file.
    pub fn upload(
        &mut self,
        tx: &mut StateTx<'_>,
        id: [u8; 32],
        size: u64,
        mime: &str,
        ts_ms: u64,
    ) -> bool {
        let prev = self.records.get(&id).cloned();
        if prev.as_ref().is_some_and(|p| p.file.is_some()) {
            return false;
        }
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
            file: None,
            charged: prev.as_ref().is_some_and(|p| p.charged),
            epoch: prev.as_ref().map_or(self.floor, |p| p.epoch),
        };
        if refs == 0 {
            let _new = self.idle.insert((ts_ms, id));
        }
        self.put(tx, id, record);
        true
    }

    /// Record a file an entry finished, unreferenced: held [`BLOB_IDLE_MS`] from `ts_ms` unless
    /// a value stored by the same entry or a later one refers to it. Not bounded by
    /// [`MAX_COMPONENT_BLOBS`]: the guest was already told it exists.
    pub fn add_file(&mut self, tx: &mut StateTx<'_>, mark: &FileMark, ts_ms: u64) {
        self.add_sealed(tx, mark, ts_ms, |_| {});
    }

    /// [`Self::add_file`] for a file `uploader` uploaded.
    pub fn add_uploaded(
        &mut self,
        tx: &mut StateTx<'_>,
        mark: &FileMark,
        ts_ms: u64,
        uploader: Option<String>,
    ) {
        self.add_sealed(tx, mark, ts_ms, |seal| seal.uploader = uploader);
    }

    /// [`Self::add_file`] for an output of a done compute run: the file may be published.
    pub fn add_output(&mut self, tx: &mut StateTx<'_>, mark: &FileMark, ts_ms: u64) {
        self.add_sealed(tx, mark, ts_ms, |seal| seal.output = true);
    }

    fn add_sealed(
        &mut self,
        tx: &mut StateTx<'_>,
        mark: &FileMark,
        ts_ms: u64,
        tag: impl FnOnce(&mut FileSeal),
    ) {
        let Some((id, mut record)) = BlobRecord::sealed(mark, ts_ms) else {
            return;
        };
        if self.records.get(&id).is_some_and(|r| r.file.is_none()) {
            return;
        }
        if let Some(seal) = record.file.as_mut() {
            tag(seal);
        }
        let _new = self.idle.insert((ts_ms, id));
        self.put(tx, id, record);
    }

    /// The sealed file `id` is an output of a done compute run, and its plaintext is `sha256`
    /// of `size` bytes.
    #[must_use]
    pub fn is_output_of(&self, id: &[u8; 32], sha256: &[u8; 32], size: u64) -> bool {
        self.records.get(id).is_some_and(|r| {
            r.size == size
                && r.file
                    .as_ref()
                    .is_some_and(|f| f.output && f.sha256.0 == sha256)
        })
    }

    /// Record the plain blob a published output made, charged and idle from `ts_ms`; one the app
    /// already holds stays as it is. Past [`MAX_COMPONENT_BLOBS`] too: the guest was told it exists.
    pub fn add_published(&mut self, tx: &mut StateTx<'_>, id: [u8; 32], size: u64, ts_ms: u64) {
        if self.records.contains_key(&id) {
            return;
        }
        let record = BlobRecord {
            size,
            mime: "application/octet-stream".to_owned(),
            refs: 0,
            idle_since_ms: ts_ms,
            file: None,
            charged: true,
            epoch: self.floor,
        };
        let _new = self.idle.insert((ts_ms, id));
        self.put(tx, id, record);
    }

    /// The owner deletes a blob: [`Self::forget`], except a sealed file, which only the
    /// retention rules forget. Answers the bytes it had charged to the quota, if it held it.
    pub fn forget_blob(&mut self, tx: &mut StateTx<'_>, id: &[u8; 32]) -> Option<u64> {
        let r = self.records.get(id).filter(|r| r.file.is_none())?;
        let charged = if r.charged { r.size } else { 0 };
        self.forget(tx, id).then_some(charged)
    }

    /// Invalidates the grants made so far for the published blob `id`; the reason it cannot.
    pub fn revoke(&mut self, tx: &mut StateTx<'_>, id: &[u8; 32]) -> Result<(), &'static str> {
        let mut r = (self.records.get(id))
            .filter(|r| r.charged)
            .cloned()
            .ok_or("no such published blob")?;
        r.epoch = r.epoch.checked_add(1).ok_or("no revocation left")?;
        self.put(tx, *id, r);
        Ok(())
    }

    /// Forget a blob; whether the app held it.
    pub fn forget(&mut self, tx: &mut StateTx<'_>, id: &[u8; 32]) -> bool {
        self.unindex_segments(id);
        let Some(prev) = Arc::make_mut(&mut self.records).remove(id) else {
            return false;
        };
        let _was = self.idle.remove(&(prev.idle_since_ms, *id));
        tx.delete_hidden(rows::CBLOB, id);
        let next = prev.epoch.checked_add(1).filter(|n| *n > self.floor);
        if let (None, Some(next)) = (&prev.file, next) {
            self.floor = next;
            match serde_json::to_vec(&self.floor) {
                Ok(v) => tx.put_hidden(rows::CBLOB, FLOOR_KEY, &v),
                Err(e) => tx.fail(e),
            }
        }
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
    /// returns them, with the size of each that was charged to the quota (a sealed file or a
    /// published blob) and the segments of a sealed file.
    pub fn expire(&mut self, tx: &mut StateTx<'_>, now_ms: u64) -> Vec<Expired> {
        let due: Vec<(u64, [u8; 32])> = self
            .idle
            .iter()
            .take_while(|(since, _)| since.saturating_add(BLOB_IDLE_MS) <= now_ms)
            .take(BLOB_EXPIRY_STEP)
            .copied()
            .collect();
        due.into_iter()
            .filter_map(|(_, id)| {
                let record = self.records.get(&id);
                let charged =
                    record.and_then(|r| (r.file.is_some() || r.charged).then_some(r.size));
                self.forget(tx, &id).then_some((id, charged))
            })
            .collect()
    }

    fn put(&mut self, tx: &mut StateTx<'_>, id: [u8; 32], record: BlobRecord) {
        match serde_json::to_vec(&record) {
            Ok(v) => tx.put_hidden(rows::CBLOB, &id, &v),
            Err(e) => tx.fail(e),
        }
        self.unindex_segments(&id);
        self.index_segments(&id, &record);
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

fn grant_message(app_id: &str, id: &[u8; 32], expires_ms: u64, epoch: u32) -> Vec<u8> {
    let epoch = if epoch == 0 {
        String::new()
    } else {
        format!("|{epoch}")
    };
    format!("{app_id}|{}|{expires_ms}{epoch}", hex::encode(id)).into_bytes()
}

/// A grant to download blob `id` of `app_id` until `expires_ms` (app time) under revocation
/// count `epoch`: `<expires_ms>[.<epoch>].<base64url HMAC>`, the epoch only when above zero.
#[must_use]
pub fn grant_token(
    key: &[u8; 32],
    app_id: &str,
    id: &[u8; 32],
    expires_ms: u64,
    epoch: u32,
) -> String {
    let mac = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, key),
        &grant_message(app_id, id, expires_ms, epoch),
    );
    let mac = calimero_identity::b64url(mac.as_ref());
    if epoch == 0 {
        format!("{expires_ms}.{mac}")
    } else {
        format!("{expires_ms}.{epoch}.{mac}")
    }
}

/// The revocation count `token` was made under, if it grants blob `id` of `app_id` at
/// `now_ms` (app time); the caller compares it with the blob's current count.
#[must_use]
pub fn grant_epoch(
    key: &[u8; 32],
    app_id: &str,
    id: &[u8; 32],
    token: &str,
    now_ms: u64,
) -> Option<u32> {
    if token.len() > MAX_GRANT_BYTES {
        return None;
    }
    let mut parts = token.split('.');
    let expires_ms = parts.next()?.parse::<u64>().ok()?;
    let (epoch, mac) = match (parts.next()?, parts.next()) {
        (mac, None) => (0, mac),
        (epoch, Some(mac)) => (epoch.parse::<u32>().ok().filter(|e| *e > 0)?, mac),
    };
    if parts.next().is_some() || now_ms > expires_ms {
        return None;
    }
    let mac = calimero_identity::from_b64url(mac).ok()?;
    hmac::verify(
        &hmac::Key::new(hmac::HMAC_SHA256, key),
        &grant_message(app_id, id, expires_ms, epoch),
        &mac,
    )
    .ok()
    .map(|()| epoch)
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
            sha256: r
                .file
                .as_ref()
                .map_or_else(|| id.to_vec(), |f| f.sha256.0.clone()),
            mime: r.mime.clone(),
        })
    }

    fn grant(&self, id: &[u8; 32], expires_ms: u64) -> String {
        let epoch = self.records.get(id).map_or(0, |r| r.epoch);
        grant_token(&self.key, &self.app_id, id, expires_ms, epoch)
    }

    fn published(&self, id: &[u8; 32]) -> bool {
        self.records.get(id).is_some_and(|r| r.charged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_holds_for_its_blob_and_app_until_it_expires() {
        let key = grant_key(&[1; 32]);
        let t = grant_token(&key, "app", &[2; 32], 1_000, 0);
        let epoch = |key, app, id: &[u8; 32], t: &str, now| grant_epoch(key, app, id, t, now);
        assert_eq!(epoch(&key, "app", &[2; 32], &t, 1_000), Some(0));
        assert_eq!(epoch(&key, "app", &[2; 32], &t, 1_001), None, "expired");
        assert_eq!(epoch(&key, "other", &[2; 32], &t, 0), None, "another app");
        assert_eq!(epoch(&key, "app", &[3; 32], &t, 0), None, "another blob");
        let forged = t.replacen("1000", "9000", 1);
        assert_eq!(
            epoch(&key, "app", &[2; 32], &forged, 0),
            None,
            "moved expiry"
        );
        assert_eq!(
            epoch(&grant_key(&[9; 32]), "app", &[2; 32], &t, 0),
            None,
            "another cluster"
        );
    }

    #[test]
    fn a_grant_names_the_revocation_count_it_was_made_under() {
        let key = grant_key(&[1; 32]);
        let old = grant_token(&key, "app", &[2; 32], 1_000, 0);
        let new = grant_token(&key, "app", &[2; 32], 1_000, 3);
        assert_eq!(grant_epoch(&key, "app", &[2; 32], &old, 0), Some(0));
        assert_eq!(grant_epoch(&key, "app", &[2; 32], &new, 0), Some(3));
        let mac = new.rsplit('.').next().unwrap();
        for forged in [
            format!("1000.4.{mac}"),
            format!("1000.0.{mac}"),
            format!("1000.{mac}"),
        ] {
            assert_eq!(
                grant_epoch(&key, "app", &[2; 32], &forged, 0),
                None,
                "{forged}"
            );
        }
        assert_eq!(
            grant_epoch(&key, "app", &[2; 32], &format!("{new}.x"), 0),
            None
        );
    }

    #[test]
    fn a_run_sees_a_file_with_its_plaintext_hash_and_no_content_type() {
        let mut records = BTreeMap::new();
        let record = |file| BlobRecord {
            size: 5,
            mime: if file {
                String::new()
            } else {
                "text/plain".into()
            },
            refs: 1,
            idle_since_ms: 0,
            file: file.then(|| FileSeal {
                sha256: Bytes(vec![9; 32]),
                wrapped_key: Bytes(vec![8; 60]),
                segments: vec![hex::encode([7; 32])],
                output: false,
                uploader: None,
            }),
            charged: false,
            epoch: 0,
        };
        let _ = records.insert([1; 32], record(true));
        let _ = records.insert([2; 32], record(false));
        let run = RunBlobs {
            app_id: "app".into(),
            records: Arc::new(records),
            key: [0; 32],
        };
        let file = run.info(&[1; 32]).unwrap();
        assert_eq!(
            (file.size, file.sha256, file.mime),
            (5, vec![9; 32], String::new())
        );
        let blob = run.info(&[2; 32]).unwrap();
        assert_eq!((blob.sha256, blob.mime), (vec![2; 32], "text/plain".into()));
        assert!(run.info(&[3; 32]).is_none());
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
