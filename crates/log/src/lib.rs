//! Calimero's replicated log: one Raft group per app instance, plus a system
//! group per cluster, all hosted by one [`Engine`] per node.
//!
//! The engine is generic over what a group replicates ([`Machine`]): it orders
//! opaque command bytes, persists them encrypted, and applies them in order on
//! every replica. Writers that must execute against the latest state (the cloud
//! node runs WASM on the leader) use [`Group::propose`], which serializes
//! proposals per group; the machine refuses, at apply, a command built on
//! stale state, so an append needs no leadership round first.

#![allow(
    clippy::result_large_err,
    reason = "openraft's trait signatures fix these error types"
)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use openraft::error::{CheckIsLeaderError, ClientWriteError, InitializeError, RaftError};
use openraft::{BasicNode, ServerState};
use tokio::sync::{watch, Mutex};

mod legacy;
pub mod machine;
#[cfg(test)]
mod migration_tests;
pub mod network;
pub mod store;
pub mod tuning;
pub mod types;

pub use legacy::MigrationReport;
pub use machine::{read_snapshot_file, Applied, Machine, Shared};
pub use network::{BoxFuture, RpcRequest, RpcResponse, Transport};
use store::{Db, LogStore};
pub use store::{GroupState, StateTx};
pub use types::{Bytes, GroupId, NodeId, Raft};

/// Timing and compaction settings shared by all groups on a node.
#[derive(Clone, Debug)]
pub struct Config {
    pub heartbeat_ms: u64,
    pub election_min_ms: u64,
    pub election_max_ms: u64,
    /// Build a snapshot after this many applied entries.
    pub snapshot_every: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            heartbeat_ms: 200,
            election_min_ms: 1200,
            election_max_ms: 2400,
            snapshot_every: 1000,
        }
    }
}

/// Why a proposal or a strong read did not complete.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// This node is not the leader; retry at `leader` if known.
    #[error("not the leader (leader: {leader:?})")]
    NotLeader { leader: Option<(NodeId, String)> },
    /// No majority, shutting down, or another transient failure. The command
    /// may or may not have been committed.
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// The command is larger than [`MAX_ENTRY_BYTES`]; nothing was appended.
    #[error("entry of {size} bytes exceeds the {max} byte limit")]
    TooLarge { size: usize, max: usize },
}

/// The largest command a log entry carries. openraft gives each append RPC one
/// heartbeat interval to finish, so a bigger entry could never reach a follower
/// and would stall replication of everything behind it. Large content (bundles)
/// goes to the blob store and the entry carries its hash.
pub const MAX_ENTRY_BYTES: usize = 1024 * 1024;

/// The size of one snapshot chunk on the wire.
pub const SNAPSHOT_CHUNK_BYTES: u64 = 1024 * 1024;

/// What a proposal builder decided.
pub enum Proposal {
    /// Append these command bytes to the log.
    Append(Vec<u8>),
    /// Answer without appending (for example, an idempotent replay).
    Reply(Vec<u8>),
}

/// The result of a proposal.
#[derive(Debug)]
pub struct Proposed {
    /// The log index of the appended entry, or the applied index for a reply.
    pub index: u64,
    /// The machine's output for the entry, or the reply bytes.
    pub output: Vec<u8>,
    /// Whether an entry was appended.
    pub appended: bool,
}

/// A snapshot of a group's Raft status.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Status {
    pub group: GroupId,
    pub leader: Option<NodeId>,
    pub is_leader: bool,
    pub term: u64,
    pub last_log: Option<u64>,
    pub applied: u64,
    pub voters: Vec<NodeId>,
    pub learners: Vec<NodeId>,
}

trait RpcTarget: Send + Sync {
    fn handle(&self, request: RpcRequest) -> BoxFuture<RpcResponse>;
    fn shutdown(&self) -> BoxFuture<()>;
}

/// One Raft group on this node.
pub struct Group<M: Machine> {
    id: GroupId,
    node: NodeId,
    raft: Raft,
    shared: Shared<M>,
    state: GroupState,
    applied_rx: watch::Receiver<u64>,
    propose_lock: Mutex<()>,
}

fn forward<E>(leader_id: Option<NodeId>, leader_node: Option<BasicNode>) -> Result<E, WriteError> {
    Err(WriteError::NotLeader {
        leader: leader_id.map(|id| (id, leader_node.map(|n| n.addr).unwrap_or_default())),
    })
}

impl<M: Machine> Group<M> {
    #[must_use]
    pub const fn id(&self) -> GroupId {
        self.id
    }

    #[must_use]
    pub fn raft(&self) -> &Raft {
        &self.raft
    }

    /// The group's durable state on this node.
    #[must_use]
    pub fn state(&self) -> &GroupState {
        &self.state
    }

    /// Form the group with these voters. Harmless if it was already formed.
    pub async fn initialize(&self, voters: BTreeMap<NodeId, String>) -> eyre::Result<()> {
        let members: BTreeMap<NodeId, BasicNode> = voters
            .into_iter()
            .map(|(id, addr)| (id, BasicNode { addr }))
            .collect();
        match self.raft.initialize(members).await {
            Ok(()) | Err(RaftError::APIError(InitializeError::NotAllowed(_))) => Ok(()),
            Err(e) => Err(eyre::eyre!("initialize group {}: {e}", self.id)),
        }
    }

    /// Confirm leadership with a majority (a heartbeat round) and wait until
    /// this node has applied everything committed before it; answers that
    /// read index.
    async fn confirm_leader(&self) -> Result<u64, WriteError> {
        match self.raft.ensure_linearizable().await {
            Ok(read) => Ok(read.map_or(0, |l| l.index)),
            Err(RaftError::APIError(CheckIsLeaderError::ForwardToLeader(f))) => {
                forward(f.leader_id, f.leader_node)
            }
            Err(e) => Err(WriteError::Unavailable(e.to_string())),
        }
    }

    /// Whether this node leads, without a majority round when its metrics
    /// already say so. Otherwise openraft's core answers: a follower is refused
    /// at once (no round trip), so it is not handed a build (a WASM run) it
    /// could not append, and a leader the metrics do not show yet confirms.
    async fn leads_here(&self) -> Result<(), WriteError> {
        if self.raft.metrics().borrow().state == ServerState::Leader {
            return Ok(());
        }
        self.confirm_leader().await.map(|_read| ())
    }

    fn build<F>(&self, build: &mut F) -> Result<(Proposal, u64), WriteError>
    where
        F: FnMut(&M, u64) -> Proposal,
    {
        let st = self
            .shared
            .read()
            .map_err(|e| WriteError::Unavailable(e.to_string()))?;
        let applied = st.applied_index();
        Ok((build(&st.machine, applied), applied))
    }

    async fn append(&self, cmd: Vec<u8>) -> Result<Proposed, WriteError> {
        let (index, output) = self.write(cmd).await?;
        Ok(Proposed {
            index,
            output,
            appended: true,
        })
    }

    /// Build a command against the latest applied state and append it.
    /// Proposals on one group run one at a time, so no other proposal lands
    /// between a build and its append.
    ///
    /// An append is not preceded by a leadership round: the machine must
    /// refuse, at apply, a command built on any state other than the one it
    /// applies to (the cloud node's `AppMachine` records the index of its own
    /// last entry as the command's `base_index`, checks it against its last
    /// entry at apply, and answers `conflict`). The `u64` handed to the
    /// builder is the log's applied index, which counts blank and membership
    /// entries too; a machine that compares with it must count them as well. A command built by a deposed or lagging leader then either
    /// never commits (`client_write` answers `NotLeader`, or a new leader
    /// truncates it) or commits as that no-op, and the caller builds again.
    /// A machine without such a check must not use `propose`.
    ///
    /// A `Reply` appends nothing, so it is a read and must be linearizable:
    /// it is confirmed with a majority before it is answered, and built again
    /// on the confirmed state when entries committed in between. `build` may
    /// therefore run twice.
    pub async fn propose<F>(&self, mut build: F) -> Result<Proposed, WriteError>
    where
        F: FnMut(&M, u64) -> Proposal,
    {
        let _guard = self.propose_lock.lock().await;
        self.leads_here().await?;
        let (proposal, built_at) = self.build(&mut build)?;
        let output = match proposal {
            Proposal::Append(cmd) => return self.append(cmd).await,
            Proposal::Reply(output) => output,
        };
        let read = self.confirm_leader().await?;
        if built_at >= read {
            return Ok(Proposed {
                index: built_at,
                output,
                appended: false,
            });
        }
        // Entries committed before the confirmation were not applied when the
        // reply was built: build again on the state the confirmation waited for.
        let (proposal, applied) = self.build(&mut build)?;
        match proposal {
            Proposal::Append(cmd) => self.append(cmd).await,
            Proposal::Reply(output) => Ok(Proposed {
                index: applied,
                output,
                appended: false,
            }),
        }
    }

    /// Append a command without reading state first.
    pub async fn write(&self, cmd: Vec<u8>) -> Result<(u64, Vec<u8>), WriteError> {
        if cmd.len() > MAX_ENTRY_BYTES {
            return Err(WriteError::TooLarge {
                size: cmd.len(),
                max: MAX_ENTRY_BYTES,
            });
        }
        match self.raft.client_write(Bytes(cmd)).await {
            Ok(r) => Ok((r.log_id.index, r.data.0)),
            Err(RaftError::APIError(ClientWriteError::ForwardToLeader(f))) => {
                forward(f.leader_id, f.leader_node)
            }
            Err(e) => Err(WriteError::Unavailable(e.to_string())),
        }
    }

    /// A linearizable read, served by the confirmed leader.
    pub async fn read_strong<R>(&self, f: impl FnOnce(&M, u64) -> R) -> Result<R, WriteError> {
        let _read = self.confirm_leader().await?;
        Ok(self.read_local(f))
    }

    /// Read this node's applied state.
    pub fn read_local<R>(&self, f: impl FnOnce(&M, u64) -> R) -> R {
        let st = self
            .shared
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&st.machine, st.applied_index())
    }

    /// The index this node has applied.
    #[must_use]
    pub fn applied(&self) -> u64 {
        *self.applied_rx.borrow()
    }

    /// Wait until this node has applied at least `index`. `false` on timeout.
    pub async fn wait_applied(&self, index: u64, timeout: Duration) -> bool {
        let mut rx = self.applied_rx.clone();
        tokio::time::timeout(timeout, async {
            loop {
                if *rx.borrow_and_update() >= index {
                    return true;
                }
                if rx.changed().await.is_err() {
                    return false;
                }
            }
        })
        .await
        .unwrap_or(false)
    }

    /// Subscribe to the applied index.
    #[must_use]
    pub fn subscribe_applied(&self) -> watch::Receiver<u64> {
        self.applied_rx.clone()
    }

    /// The current leader and its address, if known.
    pub async fn leader(&self) -> Option<(NodeId, String)> {
        let id = self.raft.current_leader().await?;
        let metrics = self.raft.metrics().borrow().clone();
        let addr = metrics
            .membership_config
            .membership()
            .get_node(&id)
            .map(|n| n.addr.clone())
            .unwrap_or_default();
        Some((id, addr))
    }

    #[must_use]
    pub fn status(&self) -> Status {
        let m = self.raft.metrics().borrow().clone();
        let membership = m.membership_config.membership();
        let voters: Vec<NodeId> = membership.voter_ids().collect();
        let learners: Vec<NodeId> = membership.learner_ids().collect();
        Status {
            group: self.id,
            leader: m.current_leader,
            is_leader: m.state == ServerState::Leader,
            term: m.current_term,
            last_log: m.last_log_index,
            applied: self.applied(),
            voters,
            learners,
        }
    }

    /// Add `id` as a learner and wait until it has caught up.
    pub async fn add_learner(&self, id: NodeId, addr: String) -> Result<(), WriteError> {
        match self.raft.add_learner(id, BasicNode { addr }, true).await {
            Ok(_) => Ok(()),
            Err(RaftError::APIError(ClientWriteError::ForwardToLeader(f))) => {
                forward(f.leader_id, f.leader_node)
            }
            Err(e) => Err(WriteError::Unavailable(e.to_string())),
        }
    }

    /// Replace the voter set (joint consensus under the hood).
    pub async fn change_voters(&self, voters: BTreeSet<NodeId>) -> Result<(), WriteError> {
        match self.raft.change_membership(voters, false).await {
            Ok(_) => Ok(()),
            Err(RaftError::APIError(ClientWriteError::ForwardToLeader(f))) => {
                forward(f.leader_id, f.leader_node)
            }
            Err(e) => Err(WriteError::Unavailable(e.to_string())),
        }
    }

    /// Hand leadership to `target` if it is a caught-up voter.
    pub async fn transfer_leader(&self, target: NodeId) -> Result<(), WriteError> {
        let status = self.status();
        if !status.voters.contains(&target) {
            return Err(WriteError::Unavailable("target is not a voter".into()));
        }
        // openraft 0.9 has no leader transfer; stepping down by triggering an
        // election elsewhere is done by the target. We ask the local node to
        // trigger nothing and report the current leader; callers retry.
        if status.leader == Some(target) {
            return Ok(());
        }
        Err(WriteError::Unavailable(
            "leader transfer is not supported yet".into(),
        ))
    }

    /// Force a snapshot now.
    pub async fn snapshot_now(&self) -> eyre::Result<()> {
        self.raft
            .trigger()
            .snapshot()
            .await
            .map_err(|e| eyre::eyre!("{e}"))
    }

    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node
    }
}

impl<M: Machine> RpcTarget for Group<M> {
    fn handle(&self, request: RpcRequest) -> BoxFuture<RpcResponse> {
        let raft = self.raft.clone();
        Box::pin(async move {
            match request {
                RpcRequest::Append(r) => match raft.append_entries(r).await {
                    Ok(r) => RpcResponse::Append(r),
                    Err(e) => RpcResponse::Error(e.to_string()),
                },
                RpcRequest::Vote(r) => match raft.vote(r).await {
                    Ok(r) => RpcResponse::Vote(r),
                    Err(e) => RpcResponse::Error(e.to_string()),
                },
                RpcRequest::Snapshot(r) => match raft.install_snapshot(r).await {
                    Ok(r) => RpcResponse::Snapshot(r),
                    Err(e) => RpcResponse::Error(e.to_string()),
                },
            }
        })
    }

    fn shutdown(&self) -> BoxFuture<()> {
        let raft = self.raft.clone();
        Box::pin(async move {
            let _ = raft.shutdown().await;
        })
    }
}

type GroupFactory = Arc<dyn Fn(GroupId) -> BoxFuture<bool> + Send + Sync>;

/// All groups hosted by one node.
pub struct Engine {
    node: NodeId,
    db: Arc<Db>,
    transport: Arc<dyn Transport>,
    config: Config,
    groups: RwLock<HashMap<GroupId, Arc<dyn RpcTarget>>>,
    factory: RwLock<Option<GroupFactory>>,
}

impl Engine {
    /// Open the node's database under `dir`, sealed under `key`.
    ///
    /// A directory still holding the previous format (`log.redb`) is migrated
    /// first; the old file is kept beside, renamed `log.redb.migrated`.
    pub fn open(
        node: NodeId,
        dir: &Path,
        key: &[u8; 32],
        transport: Arc<dyn Transport>,
        config: Config,
    ) -> eyre::Result<Arc<Self>> {
        let legacy_path = dir.join(legacy::FILE);
        let migrate = legacy_path.exists();
        let db = Db::open(dir, key)?;
        if migrate {
            let report = legacy::Legacy::open(&legacy_path, key)?.migrate_into(&db)?;
            std::fs::rename(&legacy_path, dir.join(format!("{}.migrated", legacy::FILE)))?;
            tracing::info!(?report, "migrated the log from the previous storage format");
        }
        Ok(Arc::new(Self {
            node,
            db,
            transport,
            config,
            groups: RwLock::new(HashMap::new()),
            factory: RwLock::new(None),
        }))
    }

    /// The durable state of `group` on this node, whether or not it runs here.
    #[must_use]
    pub fn group_state(&self, group: GroupId) -> GroupState {
        let generation = store::load_generation(&self.db, group).unwrap_or(0);
        GroupState {
            db: Arc::clone(&self.db),
            group,
            generation: Arc::new(std::sync::atomic::AtomicU32::new(generation)),
        }
    }

    /// The snapshot `group` serves on this node: its index, id and sealed file.
    pub fn snapshot_file(&self, group: GroupId) -> eyre::Result<Option<(u64, String, PathBuf)>> {
        machine::current_snapshot_file(&self.db, group)
    }

    /// Store a sealed blob on this node (outside every log).
    pub fn put_blob(&self, name: &str, bytes: &[u8]) -> eyre::Result<()> {
        self.db.put_blob(name, bytes)
    }

    /// Delete a blob stored on this node.
    pub fn delete_blob(&self, name: &str) -> eyre::Result<()> {
        self.db.delete_blob(name)
    }

    /// Read a blob stored on this node.
    pub fn get_blob(&self, name: &str) -> eyre::Result<Option<Vec<u8>>> {
        self.db.get_blob(name)
    }

    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node
    }

    /// Called when an RPC arrives for a group this node does not host yet; the
    /// callback starts it (returning `true`) or declines.
    pub fn set_group_factory(
        &self,
        f: impl Fn(GroupId) -> BoxFuture<bool> + Send + Sync + 'static,
    ) {
        *self
            .factory
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(f));
    }

    /// Start (or reopen) a group with `machine` as its state machine.
    ///
    /// Refuses a group this engine already hosts: two Raft instances over one
    /// group's log would each think they own it.
    pub async fn start_group<M: Machine>(
        &self,
        id: GroupId,
        machine: M,
    ) -> eyre::Result<Arc<Group<M>>> {
        if self.hosts(id) {
            eyre::bail!("group {id} is already running on this node");
        }
        let config = openraft::Config {
            cluster_name: format!("group-{id}"),
            heartbeat_interval: self.config.heartbeat_ms,
            election_timeout_min: self.config.election_min_ms,
            election_timeout_max: self.config.election_max_ms,
            snapshot_policy: openraft::SnapshotPolicy::LogsSinceLast(self.config.snapshot_every),
            max_in_snapshot_log_to_keep: 100,
            // Snapshot chunks travel as JSON RPC bodies: keep each well under
            // the node's body limit and give it time to arrive.
            snapshot_max_chunk_size: SNAPSHOT_CHUNK_BYTES,
            install_snapshot_timeout: 10_000,
            ..Default::default()
        }
        .validate()?;
        let (applied_tx, applied_rx) = watch::channel(0);
        let sm =
            machine::StateMachine::open(id, Arc::clone(&self.db), machine, Arc::new(applied_tx))?;
        let shared = sm.shared();
        let state = sm.state();
        let log = LogStore::new(id, Arc::clone(&self.db));
        let network = network::Network {
            group: id,
            transport: Arc::clone(&self.transport),
        };
        let raft = Raft::new(self.node, Arc::new(config), network, log, sm).await?;
        let group = Arc::new(Group {
            id,
            node: self.node,
            raft,
            shared,
            state,
            applied_rx,
            propose_lock: Mutex::new(()),
        });
        let _prev = self
            .groups
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, Arc::clone(&group) as Arc<dyn RpcTarget>);
        Ok(group)
    }

    #[must_use]
    pub fn hosts(&self, id: GroupId) -> bool {
        self.groups
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&id)
    }

    /// Stop a group and delete its data from this node.
    pub async fn remove_group(&self, id: GroupId) -> eyre::Result<()> {
        let target = self
            .groups
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        if let Some(t) = target {
            t.shutdown().await;
        }
        self.db.drop_group(id)
    }

    /// Stop every group (for shutdown or tests).
    pub async fn shutdown(&self) {
        let targets: Vec<_> = self
            .groups
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .map(|(_, t)| t)
            .collect();
        for t in targets {
            t.shutdown().await;
        }
    }

    /// Dispatch an RPC that arrived from a peer.
    pub async fn handle_rpc(&self, group: GroupId, request: RpcRequest) -> RpcResponse {
        let target = self
            .groups
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&group)
            .cloned();
        let target = match target {
            Some(t) => t,
            None => {
                let factory = self
                    .factory
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let started = match factory {
                    Some(f) => f(group).await,
                    None => false,
                };
                let found = self
                    .groups
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&group)
                    .cloned();
                match (started, found) {
                    (true, Some(t)) => t,
                    _ => return RpcResponse::Error(format!("group {group} is not hosted here")),
                }
            }
        };
        target.handle(request).await
    }
}
