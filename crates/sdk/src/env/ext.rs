use borsh::{to_vec as to_borsh_vec, to_vec, BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use super::{expected_boolean, expected_register, panic_str, read_register, DATA_REGISTER};
use crate::sys;
use crate::sys::{Buffer, BufferMut};

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
        amount: u128,
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
pub struct AccountId(pub String);

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
    approval: Option<ProposalId>,
}

impl DraftProposal {
    /// Create a new draft proposal.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            actions: Vec::new(),
            approval: None,
        }
    }

    /// Add an action to transfer tokens to an account.
    #[must_use]
    pub fn transfer(mut self, receiver: AccountId, amount: u128) -> Self {
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
        let actions = to_vec(&self.actions).unwrap();

        #[expect(
            clippy::needless_borrows_for_generic_args,
            reason = "We don't want to copy the buffer, but write to the same one that's returned"
        )]
        unsafe {
            sys::send_proposal(Buffer::from(&*actions), BufferMut::new(&mut buf))
        }

        ProposalId(buf)
    }
}

/// Interface for interacting with external proposals for blockchain actions.
#[derive(Clone, Copy, Debug)]
pub struct External;

impl External {
    /// Create a new proposal. This will initially be a draft, until sent.
    #[must_use]
    pub const fn propose(self) -> DraftProposal {
        DraftProposal::new()
    }

    pub fn approve(self, proposal_id: ProposalId) {
        unsafe { sys::approve_proposal(BufferMut::new(&proposal_id)) }
    }
}

/// Unique identifier for a proposal.
#[derive(
    BorshDeserialize,
    BorshSerialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
)]
pub struct ProposalId(pub [u8; 32]);

impl AsRef<[u8]> for ProposalId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[doc(hidden)]
pub unsafe fn fetch(
    url: &str,
    method: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<Vec<u8>, String> {
    let headers = match to_borsh_vec(&headers) {
        Ok(data) => data,
        Err(err) => panic_str(&format!("Cannot serialize headers: {err:?}")),
    };
    let method = Buffer::from(method);
    let url = Buffer::from(url);
    let headers = Buffer::from(headers.as_slice());
    let body = Buffer::from(body);

    let failed = unsafe { sys::fetch(url, method, headers, body, DATA_REGISTER) }
        .try_into()
        .unwrap_or_else(expected_boolean);
    let data = read_register(DATA_REGISTER).unwrap_or_else(expected_register);
    if failed {
        return Err(String::from_utf8(data).unwrap_or_else(|_| {
            panic_str("Fetch failed with an error but the error is an invalid UTF-8 string.")
        }));
    }

    Ok(data)
}
