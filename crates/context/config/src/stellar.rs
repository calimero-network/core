use soroban_sdk::{contracterror, contracttype, Address, Bytes, BytesN, String, Vec};

pub mod stellar_repr;
pub mod stellar_types;

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct StellarIdentity(pub BytesN<32>);

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct StellarProposalId(pub BytesN<32>);

#[derive(Clone, Debug)]
#[contracttype]
pub enum StellarProposalAction {
    ExternalFunctionCall(Address, String, String, i128), // receiver_id, method_name, args, deposit
    Transfer(Address, i128),                             // receiver_id, amount
    SetNumApprovals(u32),
    SetActiveProposalsLimit(u32),
    SetContextValue(Bytes, Bytes), // key, value
    DeleteProposal(BytesN<32>),    // proposal_id
}

#[derive(Clone, Debug)]
#[contracttype]
pub struct StellarProposal {
    pub id: BytesN<32>,
    pub author_id: BytesN<32>,
    pub actions: Vec<StellarProposalAction>,
}

#[derive(Clone, Debug)]
#[contracttype]
pub struct StellarProposalWithApprovals {
    pub proposal_id: BytesN<32>,
    pub num_approvals: u32,
}

#[derive(Clone, Debug)]
#[contracttype]
pub struct StellarProposalApprovalWithSigner {
    pub proposal_id: BytesN<32>,
    pub signer_id: BytesN<32>,
}

#[derive(Clone, Debug)]
#[contracttype]
pub enum StellarProxyMutateRequest {
    Propose(StellarProposal),
    Approve(StellarProposalApprovalWithSigner),
}

#[derive(Clone, Debug, Copy)]
#[contracterror]
pub enum StellarProxyError {
    AlreadyInitialized = 1,
    Unauthorized = 2,
    ProposalNotFound = 3,
    ProposalAlreadyApproved = 4,
    TooManyActiveProposals = 5,
    InvalidAction = 6,
    ExecutionFailed = 7,
    InsufficientBalance = 8,
    TransferFailed = 9,
}
