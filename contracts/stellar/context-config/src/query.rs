use soroban_sdk::{
    Address, BytesN, Env, Map, Vec, contractimpl
};
use crate::Context;
use crate::ContextContractClient;
use crate::ContextContractArgs;
use core::ops::Deref;

use crate::guard::GuardedValue;
use crate::types::{Application, Capability, Error};
use crate::ContextContract;

#[contractimpl]
impl ContextContract {
    /// Helper function to get context
    fn get_context(env: &Env, context_id: BytesN<32>) -> Option<Context> {
        Self::get_state(env).contexts.get(context_id)
    }

    /// Returns the application for a given context
    /// # Errors
    /// Returns ContextNotFound if context doesn't exist
    pub fn application(env: &Env, context_id: BytesN<32>) -> Result<Application, Error> {
        let context = Self::get_context(env, context_id)
            .ok_or(Error::ContextNotFound)?;
            
        match context.application.deref() {
            GuardedValue::Application(app) => Ok(app.clone()),
            _ => Err(Error::InvalidState),
        }
    }

    /// Returns the application revision number
    /// # Errors
    /// Returns ContextNotFound if context doesn't exist
    /// Returns InvalidState if application data is corrupted
    pub fn application_revision(env: &Env, context_id: BytesN<32>) -> Result<u64, Error> {
        let context = Self::get_context(env, context_id)
            .ok_or(Error::ContextNotFound)?;
            
        match context.application.deref() {
            GuardedValue::Application(_) => Ok(context.application.revision().into()),
            _ => Err(Error::InvalidState),
        }
    }

    /// Returns the proxy contract address
    /// # Errors
    /// Returns ContextNotFound if context doesn't exist
    pub fn proxy_contract(env: &Env, context_id: BytesN<32>) -> Result<Address, Error> {
        let context = Self::get_context(env, context_id)
            .ok_or(Error::ContextNotFound)?;
            
        match context.proxy.deref() {
            GuardedValue::Proxy(proxy_id) => Ok(proxy_id.clone()),
            _ => Err(Error::InvalidState),
        }
    }

    /// Returns a paginated list of members
    /// # Arguments
    /// * `offset` - Starting position in the members list
    /// * `length` - Number of members to return
    /// # Errors
    /// Returns ContextNotFound if context doesn't exist
    pub fn members(
        env: &Env, 
        context_id: BytesN<32>, 
        offset: u32, 
        length: u32
    ) -> Result<Vec<BytesN<32>>, Error> {
        // Validate input parameters
        if length == 0 {
            return Ok(Vec::new(env));
        }

        let context = Self::get_context(env, context_id)
            .ok_or(Error::ContextNotFound)?;
        
        let members = match context.members.deref() {
            GuardedValue::Members(members) => members,
            _ => return Err(Error::InvalidState),
        };

        let total_len = members.len();
        // Early return with empty vec for out of bounds offset
        if offset >= total_len {
            return Ok(Vec::new(env));
        }

        let end = core::cmp::min(offset + length, total_len);
        let mut result = Vec::new(env);
        
        for i in offset..end {
            result.push_back(members.get(i).unwrap().clone());
        }

        Ok(result)
    }

    /// Checks if an identity is a member of the context
    /// # Errors
    /// Returns ContextNotFound if context doesn't exist
    /// Returns InvalidState if members data is corrupted
    pub fn has_member(
        env: &Env, 
        context_id: BytesN<32>, 
        identity: BytesN<32>
    ) -> Result<bool, Error> {
        let context = Self::get_context(env, context_id)
            .ok_or(Error::ContextNotFound)?;

        match context.members.deref() {
            GuardedValue::Members(members) => Ok(members.contains(identity)),
            _ => Err(Error::InvalidState),
        }
    }

    /// Returns the revision number of the members list
    /// # Errors
    /// Returns ContextNotFound if context doesn't exist
    /// Returns InvalidState if members data is corrupted
    pub fn members_revision(env: &Env, context_id: BytesN<32>) -> Result<u64, Error> {
        let context = Self::get_context(env, context_id)
            .ok_or(Error::ContextNotFound)?;
            
        match context.members.deref() {
            GuardedValue::Members(_) => Ok(context.members.revision().into()),
            _ => Err(Error::InvalidState),
        }
    }

    /// Returns the privileges for given identities or all if identities is empty
    /// # Errors
    /// Returns ContextNotFound if context doesn't exist
    pub fn privileges(
        env: &Env,
        context_id: BytesN<32>,
        identities: Vec<BytesN<32>>,
    ) -> Result<Map<BytesN<32>, Vec<Capability>>, Error> {
        let context = Self::get_context(env, context_id)
            .ok_or(Error::ContextNotFound)?;

        let mut privileges = Map::new(env);

        // Helper function to reduce code duplication
        let add_capability = |privileges: &mut Map<BytesN<32>, Vec<Capability>>, 
                            signer_id: BytesN<32>, 
                            capability: Capability| {
            let mut caps = privileges
                .get(signer_id.clone())
                .unwrap_or_else(|| Vec::new(env));
            caps.push_back(capability);
            privileges.set(signer_id, caps);
        };

        if identities.is_empty() {
            // Process all privileges more efficiently
            for signer_id in context.application.privileged().iter() {
                add_capability(&mut privileges, signer_id, Capability::ManageApplication);
            }

            for signer_id in context.members.privileged().iter() {
                add_capability(&mut privileges, signer_id, Capability::ManageMembers);
            }

            for signer_id in context.proxy.privileged().iter() {
                add_capability(&mut privileges, signer_id, Capability::Proxy);
            }
        } else {
            // Process specific identities more efficiently
            for identity in identities.iter() {
                let mut caps = Vec::new(env);
                let id = identity.clone(); // Clone once instead of multiple times

                // Check all privileges at once
                if context.application.privileged().contains(id.clone()) {
                    caps.push_back(Capability::ManageApplication);
                }
                if context.members.privileged().contains(id.clone()) {
                    caps.push_back(Capability::ManageMembers);
                }
                if context.proxy.privileged().contains(id) {
                    caps.push_back(Capability::Proxy);
                }

                if !caps.is_empty() {
                    privileges.set(identity, caps);
                }
            }
        }

        Ok(privileges)
    }

    /// Fetches the nonce for a member in a given context
    /// # Errors
    /// Returns ContextNotFound if context doesn't exist
    /// Returns InvalidState if members data is corrupted
    /// Returns NotAMember if the provided member_id is not a member of the context
    pub fn fetch_nonce(
        env: &Env, 
        context_id: BytesN<32>, 
        member_id: BytesN<32>
    ) -> Result<Option<u64>, Error> {
        let context = Self::get_context(env, context_id)
            .ok_or(Error::ContextNotFound)?;

        // Verify member exists
        let members = match context.members.deref() {
            GuardedValue::Members(members) => members,
            _ => return Err(Error::InvalidState),
        };

        if !members.contains(&member_id) {
            return Ok(None);
        }

        // Return nonce if it exists
        Ok(context.member_nonces.get(member_id))
    }
}
