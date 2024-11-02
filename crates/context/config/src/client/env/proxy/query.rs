use crate::client::{CallClient, Error, Transport};

#[derive(Debug)]
pub struct ContextProxyQuery<'a, T> {
    client: CallClient<'a, T>,
}

type ProposalId = [u8; 32];

impl<'a, T: Transport> ContextProxyQuery<'a, T> {
    pub async fn proposals(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<Vec<ProposalId>, Error<T>> {
        todo!()
    }
}
