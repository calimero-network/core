//! Raft RPCs between nodes, over a pluggable [`Transport`].

#![allow(
    clippy::result_large_err,
    reason = "openraft's trait signatures fix these error types"
)]

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use openraft::error::{InstallSnapshotError, NetworkError, RPCError, RaftError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{AnyError, BasicNode};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::types::{GroupId, NodeId, TypeConfig};

/// A Raft RPC addressed to one group on one node.
#[derive(Debug, Serialize, Deserialize)]
pub enum RpcRequest {
    Append(AppendEntriesRequest<TypeConfig>),
    Vote(VoteRequest<NodeId>),
    Snapshot(InstallSnapshotRequest<TypeConfig>),
}

impl RpcRequest {
    /// The node the request says it comes from: the leader of an append or a
    /// snapshot, the candidate of a vote. openraft takes it on trust; a node
    /// that knows who is calling checks it against that (and against the
    /// group's members) before handing the request to the engine.
    #[must_use]
    pub fn sender(&self) -> Option<NodeId> {
        let vote = match self {
            Self::Append(r) => &r.vote,
            Self::Vote(r) => &r.vote,
            Self::Snapshot(r) => &r.vote,
        };
        vote.leader_id().voted_for()
    }
}

/// The answer to an [`RpcRequest`].
#[derive(Debug, Serialize, Deserialize)]
pub enum RpcResponse {
    Append(AppendEntriesResponse<NodeId>),
    Vote(VoteResponse<NodeId>),
    Snapshot(InstallSnapshotResponse<NodeId>),
    /// The target refused or failed; the text is for logs only.
    Error(String),
}

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Carries RPCs between nodes. The node supplies one: in-process for tests,
/// an attested channel in production.
pub trait Transport: Send + Sync + 'static {
    /// Send `request` for `group` to node `target`, reachable at `addr`.
    /// `Err` means the target could not be reached.
    fn send(
        &self,
        target: NodeId,
        addr: String,
        group: GroupId,
        request: RpcRequest,
    ) -> BoxFuture<Result<RpcResponse, String>>;

    /// Send several groups' heartbeats to node `target` as one message
    /// ([`Batcher`]); the answers come back in order, one per item. `Err`
    /// means the batch as a whole was not answered: the batcher then sends
    /// each item on its own. By default each goes on its own already.
    fn send_batch(
        &self,
        target: NodeId,
        addr: String,
        items: Vec<(GroupId, RpcRequest)>,
    ) -> BoxFuture<Result<Vec<Result<RpcResponse, String>>, String>> {
        let sends: Vec<_> = items
            .into_iter()
            .map(|(group, request)| self.send(target, addr.clone(), group, request))
            .collect();
        Box::pin(async move { Ok(join_all(sends).await) })
    }
}

/// Run every future at once; their outputs in order (a task that panicked
/// answers `Err`).
pub(crate) async fn join_all<T: Send + 'static>(
    futures: Vec<BoxFuture<Result<T, String>>>,
) -> Vec<Result<T, String>> {
    let tasks: Vec<_> = futures.into_iter().map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(tasks.len());
    for task in tasks {
        out.push(task.await.unwrap_or_else(|e| Err(e.to_string())));
    }
    out
}

/// The most heartbeats one batch carries; a fuller queue goes out at once
/// and the rest in the next batch. 1,024 empty appends are about 300 KiB of
/// JSON, well inside a peer request.
pub const MAX_HEARTBEATS_PER_BATCH: usize = 1024;

/// How long the first heartbeat for a peer waits for the others of the same
/// tick before their batch goes out. openraft gives a heartbeat one
/// `heartbeat_ms` (200 ms by default) to be answered, so this stays a small
/// part of it.
pub const HEARTBEAT_BATCH_WINDOW: Duration = Duration::from_millis(5);

type Waiter = oneshot::Sender<Result<RpcResponse, String>>;
type Queued = (GroupId, AppendEntriesRequest<TypeConfig>, Waiter);

/// One node's outbound heartbeats, gathered per peer: every group's empty
/// append to a peer within [`HEARTBEAT_BATCH_WINDOW`] travels in one
/// [`Transport::send_batch`]. Everything else goes on its own.
pub struct Batcher {
    pub(crate) transport: Arc<dyn Transport>,
    queues: Mutex<HashMap<NodeId, Vec<Queued>>>,
}

impl Batcher {
    pub(crate) fn new(transport: Arc<dyn Transport>) -> Self {
        Self {
            transport,
            queues: Mutex::new(HashMap::new()),
        }
    }

    /// Queue one group's heartbeat for `target` and wait for its answer.
    async fn heartbeat(
        self: &Arc<Self>,
        target: NodeId,
        addr: String,
        group: GroupId,
        rpc: AppendEntriesRequest<TypeConfig>,
    ) -> Result<RpcResponse, String> {
        let (tx, rx) = oneshot::channel();
        let (first, full) = {
            let mut queues = self.queues.lock().unwrap_or_else(PoisonError::into_inner);
            let queue = queues.entry(target).or_default();
            queue.push((group, rpc, tx));
            let first = queue.len() == 1;
            let full = (queue.len() >= MAX_HEARTBEATS_PER_BATCH).then(|| std::mem::take(queue));
            (first, full)
        };
        if let Some(items) = full {
            drop(tokio::spawn(Arc::clone(self).send(
                target,
                addr.clone(),
                items,
            )));
        } else if first {
            let this = Arc::clone(self);
            let addr = addr.clone();
            drop(tokio::spawn(async move {
                tokio::time::sleep(HEARTBEAT_BATCH_WINDOW).await;
                let items = this
                    .queues
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get_mut(&target)
                    .map(std::mem::take)
                    .unwrap_or_default();
                if !items.is_empty() {
                    this.send(target, addr, items).await;
                }
            }));
        }
        rx.await
            .unwrap_or_else(|_| Err("the heartbeat batch was dropped".to_owned()))
    }

    /// Send one batch and hand each group its answer. A batch the peer did
    /// not answer as a whole (unreachable, refused, the wrong number of
    /// answers) is sent again item by item, so every group gets its own
    /// answer or its own error, as without batching.
    async fn send(self: Arc<Self>, target: NodeId, addr: String, items: Vec<Queued>) {
        let requests = items
            .iter()
            .map(|(g, r, _)| (*g, RpcRequest::Append(r.clone())))
            .collect();
        let answers = match self
            .transport
            .send_batch(target, addr.clone(), requests)
            .await
        {
            Ok(answers) if answers.len() == items.len() => answers,
            other => {
                let why = other.map_or_else(|e| e, |a| format!("{} answers", a.len()));
                tracing::debug!(target, items = items.len(), %why, "heartbeat batch not answered; sending each");
                let sends = items
                    .iter()
                    .map(|(g, r, _)| {
                        self.transport
                            .send(target, addr.clone(), *g, RpcRequest::Append(r.clone()))
                    })
                    .collect();
                join_all(sends).await
            }
        };
        for ((_, _, waiter), answer) in items.into_iter().zip(answers) {
            let _gone = waiter.send(answer);
        }
    }
}

/// openraft network factory for one group.
pub struct Network {
    pub(crate) group: GroupId,
    pub(crate) batcher: Arc<Batcher>,
}

pub struct Connection {
    group: GroupId,
    target: NodeId,
    addr: String,
    batcher: Arc<Batcher>,
}

impl RaftNetworkFactory<TypeConfig> for Network {
    type Network = Connection;

    async fn new_client(&mut self, target: NodeId, node: &BasicNode) -> Self::Network {
        Connection {
            group: self.group,
            target,
            addr: node.addr.clone(),
            batcher: Arc::clone(&self.batcher),
        }
    }
}

impl Connection {
    async fn call<E: std::error::Error>(
        &self,
        request: RpcRequest,
    ) -> Result<RpcResponse, RPCError<NodeId, BasicNode, E>> {
        answered(
            self.batcher
                .transport
                .send(self.target, self.addr.clone(), self.group, request)
                .await,
        )
    }
}

fn answered<E: std::error::Error>(
    answer: Result<RpcResponse, String>,
) -> Result<RpcResponse, RPCError<NodeId, BasicNode, E>> {
    match answer {
        Ok(RpcResponse::Error(e)) => Err(RPCError::Network(NetworkError::new(&AnyError::error(e)))),
        Ok(r) => Ok(r),
        Err(e) => Err(RPCError::Unreachable(Unreachable::new(&AnyError::error(e)))),
    }
}

fn unexpected<E: std::error::Error>() -> RPCError<NodeId, BasicNode, E> {
    RPCError::Network(NetworkError::new(&AnyError::error(
        "unexpected RPC response",
    )))
}

impl RaftNetwork<TypeConfig> for Connection {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        // A heartbeat (nothing to append) joins the peer's batch; entries
        // always go on their own.
        let answer = if rpc.entries.is_empty() {
            answered(
                self.batcher
                    .heartbeat(self.target, self.addr.clone(), self.group, rpc)
                    .await,
            )?
        } else {
            self.call(RpcRequest::Append(rpc)).await?
        };
        match answer {
            RpcResponse::Append(r) => Ok(r),
            _ => Err(unexpected()),
        }
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<NodeId>,
        RPCError<NodeId, BasicNode, RaftError<NodeId, InstallSnapshotError>>,
    > {
        match self.call(RpcRequest::Snapshot(rpc)).await? {
            RpcResponse::Snapshot(r) => Ok(r),
            _ => Err(unexpected()),
        }
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<NodeId>,
        _option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        match self.call(RpcRequest::Vote(rpc)).await? {
            RpcResponse::Vote(r) => Ok(r),
            _ => Err(unexpected()),
        }
    }
}
