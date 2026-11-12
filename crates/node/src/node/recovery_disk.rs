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
            for hash in std::iter::once(&app.bundle).chain(app.compute_bundle.as_ref()) {
                pass.written +=
                    usize::from(self.write_recovery_code(&at, &holder, &to, &signer, hash)?);
            }
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
