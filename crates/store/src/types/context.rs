#![allow(single_use_lifetimes, reason = "borsh shenanigans")]

use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::{Borsh, Identity};
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

pub type Hash = [u8; 32];

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextMeta {
    pub application: key::ApplicationMeta,
    pub root_hash: Hash,
    pub dag_heads: Vec<[u8; 32]>,
}

impl ContextMeta {
    #[must_use]
    pub const fn new(
        application: key::ApplicationMeta,
        root_hash: Hash,
        dag_heads: Vec<[u8; 32]>,
    ) -> Self {
        Self {
            application,
            root_hash,
            dag_heads,
        }
    }
}

impl PredefinedEntry for key::ContextMeta {
    type Codec = Borsh;
    type DataType<'a> = ContextMeta;
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextConfig {
    pub protocol: Box<str>,
    pub network: Box<str>,
    pub contract: Box<str>,
    pub proxy_contract: Box<str>,
    pub application_revision: u64,
    pub members_revision: u64,
}

impl ContextConfig {
    #[must_use]
    pub const fn new(
        protocol: Box<str>,
        network: Box<str>,
        contract: Box<str>,
        proxy_contract: Box<str>,
        application_revision: u64,
        members_revision: u64,
    ) -> Self {
        Self {
            protocol,
            network,
            contract,
            proxy_contract,
            application_revision,
            members_revision,
        }
    }
}

impl PredefinedEntry for key::ContextConfig {
    type Codec = Borsh;
    type DataType<'a> = ContextConfig;
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextState<'a> {
    pub value: Slice<'a>,
}

impl PredefinedEntry for key::ContextState {
    type Codec = Identity;
    type DataType<'a> = ContextState<'a>;
}

impl<'a> From<Slice<'a>> for ContextState<'a> {
    fn from(value: Slice<'a>) -> Self {
        Self { value }
    }
}

impl AsRef<[u8]> for ContextState<'_> {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}

/// Node-local private storage that is NOT synchronized across nodes
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextPrivateState<'a> {
    pub value: Slice<'a>,
}

impl PredefinedEntry for key::ContextPrivateState {
    type Codec = Identity;
    type DataType<'a> = ContextPrivateState<'a>;
}

impl<'a> From<Slice<'a>> for ContextPrivateState<'a> {
    fn from(value: Slice<'a>) -> Self {
        Self { value }
    }
}

impl AsRef<[u8]> for ContextPrivateState<'_> {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "This is not expected to have additional fields"
)]
pub struct ContextIdentity {
    pub private_key: Option<[u8; 32]>,
    pub sender_key: Option<[u8; 32]>,
}

impl PredefinedEntry for key::ContextIdentity {
    type Codec = Borsh;
    type DataType<'a> = ContextIdentity;
}

/// DAG delta data (persisted)
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug)]
pub struct ContextDagDelta {
    pub delta_id: [u8; 32],
    pub parents: Vec<[u8; 32]>,
    pub actions: Vec<u8>, // Serialized actions
    pub hlc: calimero_storage::logical_clock::HybridTimestamp,
    pub applied: bool,
    pub expected_root_hash: [u8; 32],
    pub events: Option<Vec<u8>>,
}

impl ContextDagDelta {
    /// Deserialize actions from the serialized byte array
    ///
    /// # Errors
    ///
    /// Returns an error if the actions cannot be deserialized
    pub fn deserialize_actions(
        &self,
    ) -> Result<Vec<calimero_storage::action::Action>, borsh::io::Error> {
        borsh::from_slice(&self.actions)
    }

    /// Deserialize events from the serialized byte array (if present)
    ///
    /// # Errors
    ///
    /// Returns an error if the events cannot be deserialized
    #[cfg(feature = "serde")]
    pub fn deserialize_events(&self) -> Result<Option<Vec<serde_json::Value>>, eyre::Report> {
        if let Some(ref events_bytes) = self.events {
            let events: Vec<serde_json::Value> = serde_json::from_slice(events_bytes)
                .map_err(|e| eyre::eyre!("Failed to deserialize events: {}", e))?;
            Ok(Some(events))
        } else {
            Ok(None)
        }
    }
}

impl PredefinedEntry for key::ContextDagDelta {
    type Codec = Borsh;
    type DataType<'a> = ContextDagDelta;
}
