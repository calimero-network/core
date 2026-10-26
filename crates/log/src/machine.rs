//! Adapter between openraft's state machine trait and a group's [`Machine`].

#![allow(
    clippy::result_large_err,
    reason = "openraft's trait signatures fix these error types"
)]

use std::io::Cursor;
use std::sync::{Arc, RwLock};

use openraft::storage::RaftStateMachine;
use openraft::{
    AnyError, EntryPayload, OptionalSend, RaftSnapshotBuilder, Snapshot, StorageIOError,
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::store::Db;
use crate::types::{
    Bytes, Entry, GroupId, LogId, SnapshotMeta, StorageError, StoredMembership, TypeConfig,
};

/// What a group replicates. The engine calls `apply` on every node, in log
/// order, with the committed command bytes; the output goes back to the writer.
pub trait Machine: Send + Sync + 'static {
    /// Apply one committed command. Must not fail: a command that cannot be
    /// applied is recorded as a failure inside the machine's own output.
    fn apply(&mut self, index: u64, command: &[u8]) -> Vec<u8>;

    /// Serialize the whole state.
    fn snapshot(&self) -> Vec<u8>;

    /// Replace the whole state with a snapshot.
    fn restore(&mut self, data: &[u8]) -> eyre::Result<()>;
}

/// The applied state of a group, shared between the Raft core and readers.
pub struct Applied<M> {
    pub machine: M,
    pub last_applied: Option<LogId>,
    pub membership: StoredMembership,
}

impl<M> Applied<M> {
    #[must_use]
    pub fn applied_index(&self) -> u64 {
        self.last_applied.map_or(0, |l| l.index)
    }
}

pub type Shared<M> = Arc<RwLock<Applied<M>>>;

#[derive(Serialize, Deserialize)]
struct StoredSnapshot {
    meta: SnapshotMeta,
    data: Bytes,
}

fn io(e: impl std::fmt::Display) -> StorageError {
    StorageIOError::write_state_machine(AnyError::error(e.to_string())).into()
}

/// openraft state machine over a [`Machine`].
pub struct StateMachine<M: Machine> {
    group: GroupId,
    db: Arc<Db>,
    shared: Shared<M>,
    applied_tx: Arc<watch::Sender<u64>>,
    snapshots_built: u64,
}

impl<M: Machine> StateMachine<M> {
    /// Load the latest stored snapshot (if any) into `machine`.
    pub fn open(
        group: GroupId,
        db: Arc<Db>,
        mut machine: M,
        applied_tx: Arc<watch::Sender<u64>>,
    ) -> eyre::Result<Self> {
        let mut last_applied = None;
        let mut membership = StoredMembership::default();
        if let Some(snap) = db.get_meta::<StoredSnapshot>(group, "snapshot")? {
            machine.restore(&snap.data.0)?;
            last_applied = snap.meta.last_log_id;
            membership = snap.meta.last_membership.clone();
        }
        let shared = Arc::new(RwLock::new(Applied {
            machine,
            last_applied,
            membership,
        }));
        let _ = applied_tx.send(last_applied.map_or(0, |l| l.index));
        Ok(Self {
            group,
            db,
            shared,
            applied_tx,
            snapshots_built: 0,
        })
    }

    #[must_use]
    pub fn shared(&self) -> Shared<M> {
        Arc::clone(&self.shared)
    }

    fn build(&mut self) -> Result<Snapshot<TypeConfig>, StorageError> {
        let (data, meta) = {
            let st = self.shared.read().map_err(io)?;
            self.snapshots_built += 1;
            let last = st.last_applied;
            let meta = SnapshotMeta {
                last_log_id: last,
                last_membership: st.membership.clone(),
                snapshot_id: format!("{}-{}", last.map_or(0, |l| l.index), self.snapshots_built),
            };
            (st.machine.snapshot(), meta)
        };
        let stored = StoredSnapshot {
            meta: meta.clone(),
            data: Bytes(data.clone()),
        };
        self.db
            .put_meta(self.group, "snapshot", &stored)
            .map_err(io)?;
        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        })
    }
}

/// Builds snapshots off the shared state.
pub struct Builder<M: Machine> {
    inner: StateMachine<M>,
}

impl<M: Machine> RaftSnapshotBuilder<TypeConfig> for Builder<M> {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError> {
        self.inner.build()
    }
}

impl<M: Machine> RaftStateMachine<TypeConfig> for StateMachine<M> {
    type SnapshotBuilder = Builder<M>;

    async fn applied_state(&mut self) -> Result<(Option<LogId>, StoredMembership), StorageError> {
        let st = self.shared.read().map_err(io)?;
        Ok((st.last_applied, st.membership.clone()))
    }

    async fn apply<I>(&mut self, entries: I) -> Result<Vec<Bytes>, StorageError>
    where
        I: IntoIterator<Item = Entry> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        let mut out = Vec::new();
        let mut last = 0;
        {
            let mut st = self.shared.write().map_err(io)?;
            for entry in entries {
                st.last_applied = Some(entry.log_id);
                last = entry.log_id.index;
                match entry.payload {
                    EntryPayload::Blank => out.push(Bytes::default()),
                    EntryPayload::Normal(cmd) => {
                        let r = st.machine.apply(entry.log_id.index, &cmd.0);
                        out.push(Bytes(r));
                    }
                    EntryPayload::Membership(m) => {
                        st.membership = StoredMembership::new(Some(entry.log_id), m);
                        out.push(Bytes::default());
                    }
                }
            }
        }
        if last > 0 {
            let _ = self.applied_tx.send(last);
        }
        Ok(out)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        Builder {
            inner: StateMachine {
                group: self.group,
                db: Arc::clone(&self.db),
                shared: Arc::clone(&self.shared),
                applied_tx: Arc::clone(&self.applied_tx),
                snapshots_built: self.snapshots_built + 1,
            },
        }
    }

    async fn begin_receiving_snapshot(&mut self) -> Result<Box<Cursor<Vec<u8>>>, StorageError> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError> {
        let data = snapshot.into_inner();
        {
            let mut st = self.shared.write().map_err(io)?;
            st.machine.restore(&data).map_err(io)?;
            st.last_applied = meta.last_log_id;
            st.membership = meta.last_membership.clone();
        }
        let stored = StoredSnapshot {
            meta: meta.clone(),
            data: Bytes(data),
        };
        self.db
            .put_meta(self.group, "snapshot", &stored)
            .map_err(io)?;
        let _ = self
            .applied_tx
            .send(meta.last_log_id.map_or(0, |l| l.index));
        Ok(())
    }

    async fn get_current_snapshot(&mut self) -> Result<Option<Snapshot<TypeConfig>>, StorageError> {
        let Some(s) = self
            .db
            .get_meta::<StoredSnapshot>(self.group, "snapshot")
            .map_err(io)?
        else {
            return Ok(None);
        };
        Ok(Some(Snapshot {
            meta: s.meta,
            snapshot: Box::new(Cursor::new(s.data.0)),
        }))
    }
}
