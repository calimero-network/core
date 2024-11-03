use super::{ContextProxyMutate, ContextProxyMutateRequest};
use crate::repr::Repr;
use crate::types::SignerId;
use crate::{Proposal, ProposalAction, ProxyMutateRequest};

impl<'a, T> ContextProxyMutate<'a, T> {
    pub fn propose(
        self,
        author_id: SignerId,
        actions: Vec<ProposalAction>,
    ) -> ContextProxyMutateRequest<'a, T> {
        ContextProxyMutateRequest {
            client: self.client,
            raw_request: ProxyMutateRequest::Propose {
                proposal: Proposal {
                    author_id: Repr::new(author_id),
                    actions,
                },
            },
        }
    }
}
