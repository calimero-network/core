//! Writing the recovery disk (`--recovery-dir`; the format is
//! [`crate::recovery_disk`]).
//!
//! Every [`RECOVERY_EVERY`], a node writes, for each app it holds (leader or
//! not, so any one surviving disk has a whole copy of what it held), the
//! app's state at its latest snapshot, sealed to the key of whoever upgrades
//! the app ([`SystemState::backup_holder`]); the bundles those apps run; and
//! per holder its slice of the system state ([`SystemState::holder_slice`]),
//! or for the operator the whole system state. Nothing is written for a
//! holder with no key ([`SystemState::backup_keys`]); a subject whose state
//! did not move since the last file is not written again; the newest
//! [`crate::recovery_disk::KEEP`] files of each subject are kept.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use eyre::Result as EyreResult;
use serde_json::Value;

use super::Node;
use crate::recovery_disk::{self as disk, Kind, NodeSigner, Subject};
use crate::system::{BackupHolder, SystemState};

/// How often the recovery disk is written.
pub const RECOVERY_EVERY: Duration = Duration::from_secs(300);

/// How long a segment no held app lists stays on the disk: an older app
/// file kept beside the newest (`KEEP`) may still name it.
pub const SEGMENT_KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

/// The name of a sealed file's segment on the disk, beside its app's files.
/// Segments are ciphertext under each file's own data key, which is wrapped
/// under the app's file key, which only the sealed app file carries: they are
/// copied as they are, and checked against their hash when read back.
#[must_use]
pub fn segment_file(hash: &str) -> String {
    format!("seg-{hash}")
}

/// The segments a portable row of an app's state names: those of the sealed
/// file its blob record holds.
#[must_use]
pub fn segments_of_row(row: &calimero_log::PortableRow) -> Vec<String> {
    use crate::app::rows;
    use crate::component::blobs::BlobRecord;
    match row {
        calimero_log::PortableRow::Hidden { tag, key, value }
            if *tag == rows::CBLOB && key.len() == 32 =>
        {
            serde_json::from_slice::<BlobRecord>(value)
                .ok()
                .and_then(|r| r.file)
                .map(|f| f.segments)
                .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// A portable row of `app_id`'s state for a cluster whose file key is `new`:
/// a sealed file's data key, wrapped under the file key it was written
/// under (`old`, from the app file's `AppMeta`), is wrapped again.
pub fn rewrap_row(
    row: calimero_log::PortableRow,
    app_id: &str,
    old: &[u8; 32],
    new: &[u8; 32],
) -> EyreResult<calimero_log::PortableRow> {
    use crate::app::rows;
    use crate::component::blobs::BlobRecord;
    let calimero_log::PortableRow::Hidden { tag, key, value } = row else {
        return Ok(row);
    };
    if tag != rows::CBLOB || key.len() != 32 || old == new {
        return Ok(calimero_log::PortableRow::Hidden { tag, key, value });
    }
    let mut record: BlobRecord = serde_json::from_slice(&value)?;
    let Some(file) = record.file.as_mut() else {
        return Ok(calimero_log::PortableRow::Hidden { tag, key, value });
    };
    let data_key = crate::files::unwrap_key(old, app_id, &file.wrapped_key.0)?;
    file.wrapped_key = calimero_log::Bytes(crate::files::wrap_key(new, app_id, &data_key)?);
    Ok(calimero_log::PortableRow::Hidden {
        tag,
        key,
        value: serde_json::to_vec(&record)?,
    })
}

/// What the writer keeps between passes.
#[derive(Default)]
pub(crate) struct RecoveryWriter {
    /// The receipt key's attestation every file carries.
    attestation: OnceLock<Value>,
    /// The SHA-256 of the last `holder` or `system` body written, by
    /// subject, so an unchanged one is not written again.
    last: Mutex<HashMap<String, String>>,
}

/// What one pass wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecoveryPass {
    /// Files written.
    pub written: usize,
    /// Apps held here whose holder has no backup key: not recoverable
    /// without a mero-kms.
    pub without_key: usize,
}

impl Node {
    /// The attestation of this node's receipt key that its recovery files
    /// carry (`purpose=receipts` over [`disk::attestation_nonce`]).
    fn recovery_attestation(&self) -> EyreResult<Value> {
        if let Some(v) = self.recovery.attestation.get() {
            return Ok(v.clone());
        }
        let nonce = disk::attestation_nonce(&self.config.cluster, self.config.node_id);
        let governance = self.sys(SystemState::governance);
        let mut report = [0; 64];
        report[..32].copy_from_slice(&nonce);
        report[32..].copy_from_slice(&calimero_client::attest::receipt_key_binding(
            &self.receipt_public(),
            &governance,
            &self.config.cluster,
            self.config.node_id,
        ));
        let quote = crate::attest::quote(self.config.attestation, report)?;
        let doc = crate::attest::AttestationDoc {
            quote: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, quote),
            transport_key: hex::encode(self.transport.public_key()),
            app_hash: hex::encode([0_u8; 32]),
            nonce: hex::encode(nonce),
            release: self.config.attested_release(),
            cluster: self.config.cluster.clone(),
            node: self.config.node_id,
            mock: self.config.attestation == crate::attest::Mode::Mock,
            receipt_key: Some(hex::encode(self.receipt_public())),
            governance: Some(governance),
            kms: None,
            job_key: None,
            cluster_identity: None,
        };
        let v = serde_json::to_value(doc)?;
        let _set = self.recovery.attestation.set(v.clone());
        Ok(v)
    }

    fn recovery_subject(&self, kind: Kind, holder: &BackupHolder) -> Subject {
        Subject {
            kind,
            holder: holder.clone(),
            cluster: self.config.cluster.clone(),
            app: None,
            org: match holder {
                BackupHolder::Org(org) => Some(org.clone()),
                _ => None,
            },
            hash: None,
            group: 0,
            index: 0,
            written_at: crate::types::now_secs(),
            merod: env!("CARGO_PKG_VERSION").to_owned(),
            release: self.own_release(),
            node: self.config.node_id,
        }
    }

    /// Write the recovery disk once (see the module docs).
    pub async fn write_recovery(&self) -> EyreResult<RecoveryPass> {
        let mut pass = RecoveryPass::default();
        let Some(dir) = self.config.recovery_dir.clone() else {
            return Ok(pass);
        };
        let keys = self.sys(|s| s.backup_keys.clone());
        if keys.is_empty() {
            pass.without_key = self.apps.read().map(|a| a.len()).unwrap_or_default();
            return Ok(pass);
        }
        let attestation = self.recovery_attestation()?;
        let signer = NodeSigner {
            key: &self.receipt_key,
            attestation: &attestation,
        };
        let recipient = |holder: &BackupHolder| -> Option<[u8; 32]> {
            let key = keys.get(&holder.key())?;
            hex::decode(&key.public_key).ok()?.try_into().ok()
        };

        // The segments each holder's directory must keep, by directory.
        let mut listed: HashMap<std::path::PathBuf, std::collections::HashSet<String>> =
            HashMap::new();
        // Each app this node holds, at its latest snapshot.
        let groups: Vec<_> = self
            .apps
            .read()
            .map(|a| a.values().cloned().collect())
            .unwrap_or_default();
        for g in groups {
            let app_id = g.read_local(|m, _| m.app_id.clone());
            let found = self.sys(|s| {
                let app = s.apps.get(&app_id)?.clone();
                let holder = s.backup_holder(&app);
                Some((app, holder))
            });
            let Some((app, holder)) = found else {
                continue;
            };
            // A group being restored, or one this node keeps after the app
            // moved on, is not the app's state.
            if app.restore.is_some() || app.group != g.id() {
                continue;
            }
            let Some(to) = recipient(&holder) else {
                pass.without_key += 1;
                continue;
            };
            let at = disk::holder_dir(&dir, &holder);
            let Some((index, snapshot_id, file)) = self.fresh_snapshot(&g).await? else {
                continue;
            };
            let prefix = format!("app-{app_id}.");
            if !disk::files_of(&at, &format!("{prefix}{index:020}."))?.is_empty() {
                continue;
            }
            let mut subject = self.recovery_subject(Kind::App, &holder);
            subject.app = Some(app_id.clone());
            subject.group = g.id();
            subject.index = index;
            let meta = disk::AppMeta {
                record: serde_json::to_value(&app)?,
                file_key: hex::encode(crate::backup::file_key(&self.cluster_key, &app_id)),
            };
            let cluster_key = self.cluster_key;
            let group = g.id();
            let wrote = super::off_workers(|| {
                disk::write(&at, subject, &to, &signer, |out| {
                    disk::write_app_meta(out, &meta)?;
                    calimero_log::read_snapshot_rows(
                        &cluster_key,
                        group,
                        &snapshot_id,
                        &file,
                        |r| disk::write_row(out, &r),
                    )
                })
            })?;
            if wrote.is_some() {
                pass.written += 1;
            }
            let _pruned = disk::prune(&at, &prefix)?;
            let segments: Vec<String> = g.read_local(|m, _| {
                m.state
                    .component
                    .blobs
                    .records
                    .values()
                    .filter_map(|r| r.file.as_ref())
                    .flat_map(|f| f.segments.clone())
                    .collect()
            });
            for hash in &segments {
                pass.written += usize::from(self.write_recovery_segment(&at, hash)?);
            }
            listed.entry(at.clone()).or_default().extend(segments);
            for hash in std::iter::once(&app.bundle).chain(app.compute_bundle.as_ref()) {
                pass.written +=
                    usize::from(self.write_recovery_code(&at, &holder, &to, &signer, hash)?);
            }
        }

        for (at, keep) in &listed {
            let _gone = super::off_workers(|| prune_segments(at, keep))?;
        }

        // Each holder's slice of the system state; the operator's is the
        // whole state, where it upgrades apps.
        let index = self.system.applied();
        for (name, key) in &keys {
            let Some(to) = recipient(&key.holder) else {
                continue;
            };
            let (kind, body) = if key.holder == BackupHolder::Operator {
                if self.sys(|s| s.governance().app_upgrades) != crate::system::Authority::Operator {
                    continue;
                }
                (Kind::System, self.sys(serde_json::to_vec)?)
            } else {
                let slice = self.sys(|s| s.holder_slice(&key.holder));
                if slice.apps.is_empty() && slice.org.is_none() {
                    continue;
                }
                (Kind::Holder, serde_json::to_vec(&slice)?)
            };
            let digest = hex::encode(ring::digest::digest(&ring::digest::SHA256, &body));
            let seen = self
                .recovery
                .last
                .lock()
                .map(|l| l.get(name) == Some(&digest))
                .unwrap_or_default();
            if seen {
                continue;
            }
            let at = disk::holder_dir(&dir, &key.holder);
            let mut subject = self.recovery_subject(kind, &key.holder);
            subject.group = self.system.id();
            subject.index = index;
            let wrote =
                super::off_workers(|| disk::write(&at, subject, &to, &signer, |out| out(&body)))?;
            if wrote.is_some() {
                pass.written += 1;
            }
            let _pruned = disk::prune(&at, &format!("{}.", kind.name()))?;
            if let Ok(mut l) = self.recovery.last.lock() {
                let _prev = l.insert(name.clone(), digest);
            }
        }
        Ok(pass)
    }

    /// Copy segment `hash` of a sealed file into `at` once, as it is stored;
    /// answers whether it did (`false`: there already, or not held here yet).
    fn write_recovery_segment(&self, at: &Path, hash: &str) -> EyreResult<bool> {
        let path = at.join(segment_file(hash));
        if path.exists() {
            return Ok(false);
        }
        let Some(bytes) = self.engine.get_segment(hash)? else {
            return Ok(false);
        };
        let tmp = at.join(format!(".{}.tmp", segment_file(hash)));
        super::off_workers(|| {
            std::fs::create_dir_all(at)?;
            std::fs::write(&tmp, &bytes)?;
            std::fs::rename(&tmp, &path)
        })?;
        Ok(true)
    }

    /// Write bundle `hash` to `holder`'s directory once; answers whether it
    /// wrote it (`false`: there already, or not held here).
    fn write_recovery_code(
        &self,
        at: &Path,
        holder: &BackupHolder,
        to: &[u8; 32],
        signer: &NodeSigner<'_>,
        hash: &str,
    ) -> EyreResult<bool> {
        let mut subject = self.recovery_subject(Kind::Code, holder);
        subject.hash = Some(hash.to_owned());
        if at.join(disk::file_name(&subject)).exists() {
            return Ok(false);
        }
        let Some(bytes) = self.engine.get_blob(hash)? else {
            return Ok(false);
        };
        let wrote = super::off_workers(|| disk::write(at, subject, to, signer, |out| out(&bytes)))?;
        Ok(wrote.is_some())
    }
}

/// Delete the segments in `at` no held app lists (`keep`) that were written
/// more than [`SEGMENT_KEEP`] ago. Answers how many went.
fn prune_segments(at: &Path, keep: &std::collections::HashSet<String>) -> EyreResult<usize> {
    let Ok(entries) = std::fs::read_dir(at) else {
        return Ok(0);
    };
    let mut gone = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(hash) = name.strip_prefix("seg-") else {
            continue;
        };
        if keep.contains(hash) {
            continue;
        }
        let age = entry.metadata()?.modified()?.elapsed().unwrap_or_default();
        if age > SEGMENT_KEEP {
            std::fs::remove_file(entry.path())?;
            gone += 1;
        }
    }
    Ok(gone)
}
