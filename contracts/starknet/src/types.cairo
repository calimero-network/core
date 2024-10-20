#[derive(Drop, Serde, starknet::Store)]
pub type ContextId = felt252;

// Context Member ID
#[derive(Drop, Serde, Debug, starknet::Store)]
pub type ContextIdentity = felt252;

// Context
#[derive(Drop, Serde, starknet::Store)]
pub struct Context {
    pub application: Application,
    pub member_count: u32,
}

// Context Application
#[derive(Drop, Serde, Debug, starknet::Store)]
pub struct Application {
    pub id: felt252,  // Represents [u8; 32]
    pub blob: felt252,  // Represents [u8; 32]
    pub size: u64,
    pub source: ByteArray,  // Represents ApplicationSource
    pub metadata: ByteArray,  // Represents ApplicationMetadata
}

// Context Config
#[derive(Drop, Serde, starknet::Store)]
pub struct Config {
    pub validity_threshold_ms: u64,
}

// #[derive(Drop, Serde)]
// pub struct ContextDetails {
//     pub context_id: felt252,  // Represents [u8; 32]
//     pub application: Application,
//     pub member_count: u32,
//     pub members: Array<ContextIdentity>,
// }

// Context Capabilities
#[derive(Drop, Serde, PartialEq, Copy, Debug)]
pub enum Capability {
    ManageApplication,
    ManageMembers,
}

// Convert Capability to felt252
impl CapabilityIntoFelt252 of Into<Capability, felt252> {
    fn into(self: Capability) -> felt252 {
        match self {
            Capability::ManageApplication => 0,
            Capability::ManageMembers => 1,
        }
    }
}

// Convert felt252 to Capability
impl Felt252TryIntoCapability of TryInto<felt252, Capability> {
    fn try_into(self: felt252) -> Option<Capability> {
        match self {
            0 => Option::Some(Capability::ManageApplication),
            1 => Option::Some(Capability::ManageMembers),
            _ => Option::None,
        }
    }
}

#[derive(Drop, Serde)]
pub struct Signed<T> {
    pub payload: Array<felt252>,
    pub signature: (felt252, felt252),  // (r, s) of the signature
    pub public_key: felt252,
}

#[derive(Drop, Serde)]
pub struct Request {
    pub kind: RequestKind,
    pub signer_id: ContextIdentity,
    pub timestamp_ms: u64,
}

#[derive(Drop, Serde)]
pub enum RequestKind {
    Context: ContextRequest,
}

#[derive(Drop, Serde)]
pub struct ContextRequest {
    pub context_id: ContextId,
    pub kind: ContextRequestKind,
}

#[derive(Drop, Serde)]
pub enum ContextRequestKind {
    Add: (ContextIdentity, Application),
    UpdateApplication: Application,
    AddMembers: Array<ContextIdentity>,
    RemoveMembers: Array<ContextIdentity>,
    Grant: Array<(ContextIdentity, Capability)>,
    Revoke: Array<(ContextIdentity, Capability)>,
}



// Events
// #[event]
// #[derive(Drop, starknet::Event)]
// struct MemberRemoved {
//     context_id: felt252,
//     member_id: ContextIdentity
// }

// #[event]
// #[derive(Drop, starknet::Event)]
// struct MemberAdded {
//     context_id: felt252,
//     member_id: ContextIdentity
// }

// #[event]
// #[derive(Drop, starknet::Event)]
// struct ApplicationUpdated {
//     context_id: felt252,
//     old_application_id: felt252,
//     new_application_id: felt252
// }

// #[event]
// #[derive(Drop, starknet::Event)]
// struct CapabilityGranted {
//     context_id: felt252,
//     member_id: ContextIdentity,
//     capability: Capability
// }
