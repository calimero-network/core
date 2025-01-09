use core::fmt;
use std::borrow::Cow;
use std::str::FromStr;

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
        deposit: u128,
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

    /// Delete a proposal.
    DeleteProposal {
        /// The ID of the proposal to delete.
        proposal_id: ProposalId,
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

    // Add an action to call an external function
    #[must_use]
    pub fn external_function_call(
        mut self,
        receiver_id: String,
        method_name: String,
        args: String,
        deposit: u128,
    ) -> Self {
        self.actions.push(ProposalAction::ExternalFunctionCall {
            receiver_id: AccountId(receiver_id),
            method_name,
            args,
            deposit,
        });
        self
    }

    /// Add an action to set a context value
    #[must_use]
    pub fn set_context_value(mut self, key: Box<[u8]>, value: Box<[u8]>) -> Self {
        self.actions
            .push(ProposalAction::SetContextValue { key, value });
        self
    }

    /// Add an action to set number of approvals
    #[must_use]
    pub fn set_num_approvals(mut self, num_approvals: u32) -> Self {
        self.actions
            .push(ProposalAction::SetNumApprovals { num_approvals });
        self
    }

    /// Add an action to set active proposals limit
    #[must_use]
    pub fn set_active_proposals_limit(mut self, active_proposals_limit: u32) -> Self {
        self.actions.push(ProposalAction::SetActiveProposalsLimit {
            active_proposals_limit,
        });
        self
    }

    /// Add an action to delete a proposal.
    #[must_use]
    pub fn delete(mut self, proposal_id: ProposalId) -> Self {
        self.actions
            .push(ProposalAction::DeleteProposal { proposal_id });
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
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProposalId(pub [u8; 32]);

impl AsRef<[u8]> for ProposalId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl ProposalId {
    pub fn as_str<'a>(&self, buf: &'a mut [u8; 44]) -> &'a str {
        let len = bs58::encode(&self.0).onto(&mut buf[..]).unwrap();
        std::str::from_utf8(&buf[..len]).unwrap()
    }
}

impl FromStr for ProposalId {
    type Err = bs58::decode::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut buf = [0; 32];
        let _len = bs58::decode(s).onto(&mut buf[..])?;
        Ok(Self(buf))
    }
}

impl Serialize for ProposalId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut buf = [0; 44];
        serializer.serialize_str(self.as_str(&mut buf))
    }
}

impl<'de> Deserialize<'de> for ProposalId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Container<'a>(#[serde(borrow)] Cow<'a, str>);

        let encoded = Container::deserialize(deserializer)?;
        Self::from_str(&*encoded.0).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for ProposalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str(&mut [0; 44]))
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
