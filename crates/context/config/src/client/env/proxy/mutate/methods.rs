use super::{ContextProxyMutate, ContextProxyMutateRequest};
use crate::repr::Repr;
use crate::types::{ProposalId, SignerId};
use crate::{Proposal, ProposalAction, ProposalApprovalWithSigner, ProxyMutateRequest};

impl<'a, T> ContextProxyMutate<'a, T> {
    pub fn propose(
        self,
        proposal_id: ProposalId,
        author_id: SignerId,
        actions: Vec<ProposalAction>,
    ) -> ContextProxyMutateRequest<'a, T> {
        ContextProxyMutateRequest {
            client: self.client,
            raw_request: ProxyMutateRequest::Propose {
                proposal: Proposal {
                    id: Repr::new(proposal_id),
                    author_id: Repr::new(author_id),
                    actions,
                },
            },
        }
    }

    pub fn approve(
        self,
        signer_id: SignerId,
        proposal_id: ProposalId,
    ) -> ContextProxyMutateRequest<'a, T> {
        ContextProxyMutateRequest {
            client: self.client,
            raw_request: ProxyMutateRequest::Approve {
                approval: ProposalApprovalWithSigner {
                    proposal_id: Repr::new(proposal_id),
                    signer_id: Repr::new(signer_id),
                    added_timestamp: 0, // TODO: add timestamp
                },
            },
        }
    }
}
