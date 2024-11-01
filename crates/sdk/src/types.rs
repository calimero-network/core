use core::error::Error as CoreError;
use borsh::{to_vec, BorshDeserialize, BorshSerialize};
use serde::{Serialize, Serializer};
use crate::env;

#[derive(Debug, Serialize)]
pub struct Error(#[serde(serialize_with = "error_string")] Box<dyn CoreError>);

fn error_string<S>(error: &impl AsRef<dyn CoreError>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&error.as_ref().to_string())
}

impl Error {
    #[must_use]
    pub fn msg(s: &str) -> Self {
        Self(s.to_owned().into())
    }
}

impl<T> From<T> for Error
where
    T: CoreError + 'static,
{
    fn from(error: T) -> Self {
        Self(Box::new(error))
    }
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AccountId(String);

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, PartialEq)]
pub enum ProposalAction {
    ExternalFunctionCall {
        receiver_id: AccountId,
        method_name: String,
        args: String,
        deposit: u64,
        gas: u64,
    },
    Transfer {
        receiver_id: AccountId,
        amount: u64,
    },
    SetNumApprovals {
        num_approvals: u32,
    },
    SetActiveProposalsLimit {
        active_proposals_limit: u32,
    },
    SetContextValue {
        key: Box<[u8]>,
        value: Box<[u8]>,
    },
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProposalId(pub u32);

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Default, PartialEq)]
pub struct DraftProposal {
    actions: Vec<ProposalAction>,
}

impl DraftProposal {
    pub fn transfer(mut self, receiver: AccountId, amount: u64) -> Self {
        self.actions.push(ProposalAction::Transfer {
            receiver_id: receiver,
            amount,
        });
        self
    }
    
    pub fn set(mut self, key: Vec<u8>, value: Vec<u8>) -> Self {
        self.actions.push(ProposalAction::SetContextValue {
            key: key.into(),
            value: value.into(),
        });
        self
    }
    
    pub fn send(self) -> ProposalId {
        env::send_proposal(&to_vec(&self.actions).unwrap())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct External {}

impl External {
    pub fn propose(self) -> DraftProposal {
        DraftProposal::default()
    }
}

