use super::types::{Signed, Request, ContextId, Application, ContextIdentity, Capability};

#[starknet::interface]
pub trait IContextConfigs<TContractState> {
    fn application(self: @TContractState, context_id: ContextId) -> Application;
    fn members(self: @TContractState, context_id: ContextId, offset: u32, length: u32) -> Array<ContextIdentity>;
    fn privileges(self: @TContractState, context_id: ContextId, identities: Array<ContextIdentity>) -> Array<(ContextIdentity, Array<Capability>)>;
    fn mutate(ref self: TContractState, signed_request: Signed<Request>);
}
