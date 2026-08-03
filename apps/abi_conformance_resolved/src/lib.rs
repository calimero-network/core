//! Conformance coverage for types only the compiler can resolve.
//!
//! An alias, a macro-generated type, a multi-segment path and a re-export: each
//! is a name no source reader could follow back to a definition. The `AbiType`
//! impls the compiler picks describe all four exactly, which is what this app
//! exists to prove.

use calimero_sdk::abi::AbiType;
use calimero_sdk::app;
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_storage::collections::{LwwRegister, UnorderedMap};

mod inner;

/// Reaches the ABI as the `string` it aliases, not as a type named `MessageId`.
pub type MessageId = String;

/// A 32-byte id minted by a macro, the shape `id::define!` generates.
macro_rules! define_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Serialize, Deserialize, AbiType)]
        #[serde(crate = "calimero_sdk::serde")]
        pub struct $name([u8; 32]);
    };
}

define_id!(RoomId);

/// The definition is `inner::Real`; nothing outside this line spells that name.
pub use self::inner::Real as Renamed;

#[app::state]
pub struct ResolvedConformance {
    messages: UnorderedMap<String, LwwRegister<String>>,
}

#[app::logic]
impl ResolvedConformance {
    #[app::init]
    pub fn init() -> ResolvedConformance {
        ResolvedConformance {
            messages: UnorderedMap::new(),
        }
    }

    /// An alias on both sides of the signature.
    pub fn record(&mut self, id: MessageId, body: String) -> app::Result<MessageId> {
        self.messages.insert(id.clone(), LwwRegister::new(body))?;
        Ok(id)
    }

    /// A macro-generated newtype as a parameter.
    pub fn join(&mut self, room: RoomId) -> app::Result<()> {
        self.messages
            .insert(hex(&room), LwwRegister::new("joined".to_owned()))?;
        Ok(())
    }

    /// A multi-segment path at the use site.
    pub fn tally(&self, counts: std::collections::BTreeMap<String, u32>) -> app::Result<u32> {
        Ok(counts.values().sum())
    }

    /// A type reachable only under its re-exported name.
    pub fn describe(&self, what: Renamed) -> app::Result<String> {
        Ok(what.tag)
    }
}

fn hex(room: &RoomId) -> String {
    room.0.iter().map(|b| format!("{b:02x}")).collect()
}
