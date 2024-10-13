use std::collections::VecDeque;

use calimero_network::types::PeerId;
use calimero_node_primitives::MutateCallError;
use calimero_runtime::logic::Outcome;
use calimero_storage::interface::Action;
use tokio::sync::oneshot;

#[derive(Debug)]
#[non_exhaustive]
pub struct ActionPoolEntry {
    pub sender: PeerId,
    pub actions: Vec<Action>,
    pub outcome_sender: Option<oneshot::Sender<Result<Outcome, MutateCallError>>>,
}

#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ActionPool {
    pub actions: VecDeque<ActionPoolEntry>,
}

impl ActionPool {
    pub fn insert(
        &mut self,
        sender: PeerId,
        actions: Vec<Action>,
        outcome_sender: Option<oneshot::Sender<Result<Outcome, MutateCallError>>>,
    ) {
        self.actions.push_back(ActionPoolEntry {
            sender,
            actions,
            outcome_sender,
        });
    }

    pub fn iter(&self) -> impl Iterator<Item = &ActionPoolEntry> {
        self.actions.iter()
    }
}
