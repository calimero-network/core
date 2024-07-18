use std::collections::BTreeMap;

use tokio::sync::oneshot;

#[derive(Debug)]
pub struct TransactionPoolEntry {
    pub sender: calimero_network::types::PeerId,
    pub transaction: calimero_primitives::transaction::Transaction,
    pub outcome_sender: Option<
        oneshot::Sender<
            Result<calimero_runtime::logic::Outcome, calimero_node_primitives::MutateCallError>,
        >,
    >,
}

#[derive(Debug, Default)]
pub struct TransactionPool {
    pub transactions: BTreeMap<calimero_primitives::hash::Hash, TransactionPoolEntry>,
}

impl TransactionPool {
    pub fn insert(
        &mut self,
        sender: calimero_network::types::PeerId,
        transaction: calimero_primitives::transaction::Transaction,
        outcome_sender: Option<
            oneshot::Sender<
                Result<calimero_runtime::logic::Outcome, calimero_node_primitives::MutateCallError>,
            >,
        >,
    ) -> eyre::Result<calimero_primitives::hash::Hash> {
        let transaction_hash = calimero_primitives::hash::Hash::hash_json(&transaction)
            .expect("Failed to hash transaction. This is a bug and should be reported.");

        self.transactions.insert(
            transaction_hash,
            TransactionPoolEntry {
                sender,
                transaction,
                outcome_sender,
            },
        );

        Ok(transaction_hash)
    }

    pub fn remove(
        &mut self,
        hash: &calimero_primitives::hash::Hash,
    ) -> Option<TransactionPoolEntry> {
        self.transactions.remove(hash)
    }

    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (&calimero_primitives::hash::Hash, &TransactionPoolEntry)> {
        self.transactions.iter()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }
}
