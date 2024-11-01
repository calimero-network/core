use borsh::{to_vec, BorshDeserialize, BorshSerialize};

use crate::env;

/// A blockchain proposal action.
///
/// This enum represents the different actions that can be executed against a
/// blockchain, and combined into a proposal.
///
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub enum ProposalAction {
    /// Call a method on a contract.
    ExternalFunctionCall {
        /// The account ID of the contract to call.
        receiver_id: AccountId,

        /// The method name to call.
        method_name: String,

        /// The arguments to pass to the method.
        args: String,

        /// The amount of tokens to attach to the call.
        deposit: u64,

        /// The maximum amount of gas to use for the call.
        gas: u64,
    },

    /// Transfer tokens to an account.
    Transfer {
        /// The account ID of the receiver.
        receiver_id: AccountId,

        /// The amount of tokens to transfer.
        amount: u64,
    },

    /// Set the number of approvals required for a proposal to be executed.
    SetNumApprovals {
        /// The number of approvals required.
        num_approvals: u32,
    },

    /// Set the number of active proposals allowed at once.
    SetActiveProposalsLimit {
        /// The number of active proposals allowed.
        active_proposals_limit: u32,
    },

    /// Set a value in the contract's context.
    SetContextValue {
        /// The key to set.
        key: Box<[u8]>,

        /// The value to set.
        value: Box<[u8]>,
    },
}

/// Unique identifier for an account.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AccountId(String);

/// A draft proposal.
///
/// This struct is used to build a proposal before sending it to the blockchain.
/// It is distinct from a proposal that has been prepared and needs signing.
///
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct DraftProposal {
    /// The actions to be executed by the proposal. One proposal can contain
    /// multiple actions to execute.
    actions: Vec<ProposalAction>,
}

impl DraftProposal {
    /// Add an action to transfer tokens to an account.
    #[must_use]
    pub fn transfer(mut self, receiver: AccountId, amount: u64) -> Self {
        self.actions.push(ProposalAction::Transfer {
            receiver_id: receiver,
            amount,
        });
        self
    }

    /// Finalise the proposal and send it to the blockchain.
    #[must_use]
    pub fn send(self) -> ProposalId {
        let mut buf = [0; 32];
        env::send_proposal(&to_vec(&self.actions).unwrap(), &mut buf);
        ProposalId(buf)
    }
}

/// Interface for interacting with external proposals for blockchain actions.
#[derive(Clone, Copy, Debug)]
pub struct External;

impl External {
    /// Create a new proposal. This will initially be a draft, until sent.
    #[must_use]
    pub fn propose(self) -> DraftProposal {
        DraftProposal::default()
    }
}

/// Unique identifier for a proposal.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProposalId(pub [u8; 32]);
