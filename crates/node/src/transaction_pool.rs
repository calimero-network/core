use std::collections::BTreeMap;

use calimero_node_primitives::MutateCallError;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PeerId;
use calimero_primitives::transaction::Transaction;
use calimero_runtime::logic::Outcome;
use eyre::{eyre, Result as EyreResult};
use tokio::sync::oneshot;

#[derive(Debug)]
#[non_exhaustive]
pub struct TransactionPoolEntry {
    pub sender: PeerId,
    pub transaction: Transaction,
    pub outcome_sender: Option<oneshot::Sender<Result<Outcome, MutateCallError>>>,
}

#[derive(Debug, Default)]
#[non_exhaustive]
pub struct TransactionPool {
    pub transactions: BTreeMap<Hash, TransactionPoolEntry>,
}

impl TransactionPool {
    pub fn insert(
        &mut self,
        sender: PeerId,
        transaction: Transaction,
        outcome_sender: Option<oneshot::Sender<Result<Outcome, MutateCallError>>>,
    ) -> EyreResult<Hash> {
        let transaction_hash = Hash::hash_json(&transaction).map_err(|err| {
            eyre!("Failed to hash transaction: {err}. This is a bug and should be reported.")
        })?;

        drop(self.transactions.insert(
            transaction_hash,
            TransactionPoolEntry {
                sender,
                transaction,
                outcome_sender,
            },
        ));

        Ok(transaction_hash)
    }

    pub fn remove(&mut self, hash: &Hash) -> Option<TransactionPoolEntry> {
        self.transactions.remove(hash)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Hash, &TransactionPoolEntry)> {
        self.transactions.iter()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }
}
