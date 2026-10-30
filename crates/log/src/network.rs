//! Raft RPCs between nodes, over a pluggable [`Transport`].

#![allow(
    clippy::result_large_err,
    reason = "openraft's trait signatures fix these error types"
)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use openraft::error::{InstallSnapshotError, NetworkError, RPCError, RaftError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{AnyError, BasicNode};
use serde::{Deserialize, Serialize};

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
}

/// openraft network factory for one group.
pub struct Network {
    pub(crate) group: GroupId,
    pub(crate) transport: Arc<dyn Transport>,
}

pub struct Connection {
    group: GroupId,
    target: NodeId,
    addr: String,
    transport: Arc<dyn Transport>,
}

impl RaftNetworkFactory<TypeConfig> for Network {
    type Network = Connection;

    async fn new_client(&mut self, target: NodeId, node: &BasicNode) -> Self::Network {
        Connection {
            group: self.group,
            target,
            addr: node.addr.clone(),
            transport: Arc::clone(&self.transport),
        }
    }
}

impl Connection {
    async fn call<E: std::error::Error>(
        &self,
        request: RpcRequest,
    ) -> Result<RpcResponse, RPCError<NodeId, BasicNode, E>> {
        match self
            .transport
            .send(self.target, self.addr.clone(), self.group, request)
            .await
        {
            Ok(RpcResponse::Error(e)) => {
                Err(RPCError::Network(NetworkError::new(&AnyError::error(e))))
            }
            Ok(r) => Ok(r),
            Err(e) => Err(RPCError::Unreachable(Unreachable::new(&AnyError::error(e)))),
        }
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
        match self.call(RpcRequest::Append(rpc)).await? {
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
