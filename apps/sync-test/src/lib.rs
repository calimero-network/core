//! Sync test app — minimal app for 3-node sync scenarios.
//!
//! Designed to reproduce the Sandi/Matea/Fran session:
//! - Multiple nodes writing concurrently
//! - State convergence verification
//! - DM-like invitation flow: one node writes an "invitation" that another must read

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};
use thiserror::Error;

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct SyncTest {
    /// Key-value entries written by different nodes.
    entries: UnorderedMap<String, LwwRegister<String>>,
    /// Tracks which node wrote each key (node_alias -> list of keys).
    writers: UnorderedMap<String, LwwRegister<String>>,
    /// Simulates DM invitations: inviter stores an invitation that the invitee must read.
    /// Key: "inviter:invitee" (e.g., "sandi:matea"), Value: invitation payload (e.g., context_id).
    invitations: UnorderedMap<String, LwwRegister<String>>,
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("key not found: {0}")]
    NotFound(&'a str),
}

#[app::logic]
impl SyncTest {
    #[app::init]
    pub fn init() -> SyncTest {
        SyncTest {
            entries: UnorderedMap::new_with_field_name("entries"),
            writers: UnorderedMap::new_with_field_name("writers"),
            invitations: UnorderedMap::new_with_field_name("invitations"),
        }
    }

    /// Write a key-value pair, tagged with the writer's alias.
    pub fn write(&mut self, key: String, value: String, writer: String) -> app::Result<()> {
        app::log!("[{}] write: {} = {}", writer, key, value);

        self.entries.insert(key.clone(), value.into())?;

        let writer_key = format!("{}:{}", writer, key);
        self.writers.insert(writer_key, writer.into())?;

        Ok(())
    }

    /// Read a single key.
    pub fn read(&self, key: &str) -> app::Result<Option<String>> {
        Ok(self.entries.get(key)?.map(|v| v.get().clone()))
    }

    /// Get all entries as a sorted map — used to verify convergence across nodes.
    pub fn snapshot(&self) -> app::Result<BTreeMap<String, String>> {
        Ok(self
            .entries
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    /// Get the total number of entries.
    pub fn count(&self) -> app::Result<usize> {
        Ok(self.entries.len()?)
    }

    // ========================================
    // DM invitation simulation
    // ========================================

    /// Node A creates a "DM invitation" for Node B.
    /// This simulates Curb's create_dm_chat: the inviter writes invitation
    /// metadata into the shared main context state. The invitee must read
    /// this from the synced state to "discover" the DM.
    pub fn create_invitation(
        &mut self,
        inviter: String,
        invitee: String,
        payload: String,
    ) -> app::Result<()> {
        let key = format!("{}:{}", inviter, invitee);
        app::log!("[{}] creating invitation for {}: {}", inviter, invitee, payload);
        self.invitations.insert(key, payload.into())?;
        Ok(())
    }

    /// Node B reads an invitation addressed to them.
    /// Returns None if the invitation hasn't synced yet.
    pub fn read_invitation(
        &self,
        inviter: &str,
        invitee: &str,
    ) -> app::Result<Option<String>> {
        let key = format!("{}:{}", inviter, invitee);
        Ok(self.invitations.get(&key)?.map(|v| v.get().clone()))
    }

    /// Get all pending invitations — for debugging convergence.
    pub fn all_invitations(&self) -> app::Result<BTreeMap<String, String>> {
        Ok(self
            .invitations
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }
}
