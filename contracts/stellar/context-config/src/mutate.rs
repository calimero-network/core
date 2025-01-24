use soroban_sdk::{
  contractimpl, Address, BytesN, Env, IntoVal, Map, Symbol, Vec
};
use core::ops::{Deref, DerefMut};

use crate::types::{
  Application, Capability, Error, SignedRequest, 
  RequestKind, ContextRequest, ContextRequestKind
};
use crate::guard::{Guard, GuardedValue};
use crate::{Context, ContextContract};
use crate::ContextContractClient;
use crate::ContextContractArgs;

#[contractimpl]
impl ContextContract {
  /// Process a signed mutation request
  /// # Errors
  /// Returns InvalidSignature if request signature is invalid
  /// Returns InvalidNonce if nonce is incorrect
  /// Returns various context-specific errors based on the request kind
  pub fn mutate(env: Env, signed_request: SignedRequest) -> Result<(), Error> {
      // Verify signature and get request
      let request = signed_request.verify(&env)?;
      // Extract context_id and kind from request
      let (context_id, kind) = match request.kind {
          RequestKind::Context(ContextRequest { context_id, kind }) => (context_id, kind),
      };

      // Check and increment nonce
      Self::check_and_increment_nonce(
          &env,
          &context_id,
          &request.signer_id,
          request.nonce,
      )?;
      
      match kind {
          ContextRequestKind::Add(author_id, application) => {
              Self::add_context(&env, &request.signer_id, &context_id, &author_id, &application)
          },
          ContextRequestKind::UpdateApplication(application) => {
              Self::update_application(&env, &request.signer_id, &context_id, &application)
          },
          ContextRequestKind::AddMembers(members) => {
              Self::add_members(&env, &request.signer_id, &context_id, &members)
          },
          ContextRequestKind::RemoveMembers(members) => {
              Self::remove_members(&env, &request.signer_id, &context_id, &members)
          },
          ContextRequestKind::Grant(capabilities) => {
              Self::grant(&env, &request.signer_id, &context_id, &capabilities)
          },
          ContextRequestKind::Revoke(capabilities) => {
              Self::revoke(&env, &request.signer_id, &context_id, &capabilities)
          },
          ContextRequestKind::UpdateProxyContract => {
              Self::update_proxy_contract(&env, &request.signer_id, &context_id)
          }
      }
  }

  /// Validates and increments the nonce for a member's request
  /// # Errors
  /// Returns InvalidNonce if the provided nonce doesn't match the current value
  fn check_and_increment_nonce(
      env: &Env,
      context_id: &BytesN<32>,
      member_id: &BytesN<32>,
      nonce: u64,
  ) -> Result<(), Error> {
      Self::update_state(env, |state| {
          // If context doesn't exist yet, allow the operation
          let Some(context) = state.contexts.get(context_id.clone()) else {
              return Ok(());
          };

          // If member doesn't have a nonce yet, allow the operation
          let Some(current_nonce) = context.member_nonces.get(member_id.clone()) else {
              // Only set the nonce if it's a new context operation
              if nonce == 0 {
                  let mut updated_context = context.clone();
                  updated_context.member_nonces.set(member_id.clone(), 1);
                  state.contexts.set(context_id.clone(), updated_context);
              }
              return Ok(());
          };

          // For existing members, verify and increment nonce
          if current_nonce != nonce {
              return Err(Error::InvalidNonce);
          }

          let mut updated_context = context.clone();
          updated_context.member_nonces.set(member_id.clone(), nonce + 1);
          state.contexts.set(context_id.clone(), updated_context);
          
          Ok(())
      })
  }

  /// Adds a new context with initial application and author
  /// # Errors
  /// Returns Unauthorized if signer is not the context itself
  /// Returns ContextExists if context already exists
  fn add_context(
      env: &Env,
      signer_id: &BytesN<32>,
      context_id: &BytesN<32>,
      author_id: &BytesN<32>,
      application: &Application,
  ) -> Result<(), Error> {
      // Verify that the signer is the context itself
      if signer_id.as_ref() != context_id.as_ref() {
          return Err(Error::Unauthorized);
      }

      Self::update_state(env, |state| {
          // Check if context already exists
          if state.contexts.contains_key(context_id.clone()) {
              return Err(Error::ContextExists);
          }

          // Deploy proxy contract
          let proxy_address = Self::deploy_proxy(env, context_id)?;

          // Initialize members vector
          let mut members = Vec::new(env);
          members.push_back(author_id.clone());

          // Initialize member nonces
          let mut member_nonces = Map::new(env);
          member_nonces.set(author_id.clone(), 0);

          // Create context with all components
          let context = Context {
              application: Guard::new(
                  env,
                  &author_id,
                  GuardedValue::Application(application.clone())
              ),
              members: Guard::new(
                  env,
                  &author_id,
                  GuardedValue::Members(members)
              ),
              proxy: Guard::new(
                  env,
                  &author_id,
                  GuardedValue::Proxy(proxy_address)
              ),
              member_nonces,
          };

          state.contexts.set(context_id.clone(), context);
          Ok(())
      })
  }

  /// Updates the application configuration for a context
  /// # Errors
  /// Returns ContextNotFound if context doesn't exist
  /// Returns Unauthorized if signer doesn't have ManageApplication capability
  /// Returns InvalidState if application data is corrupted
  fn update_application(
      env: &Env,
      signer_id: &BytesN<32>,
      context_id: &BytesN<32>,
      application: &Application,
  ) -> Result<(), Error> {
      Self::update_state(env, |state| {
          let context = state.contexts
              .get(context_id.clone())
              .ok_or(Error::ContextNotFound)?;

          let mut updated_context = context.clone();

          // Get application guard and verify permissions
          let guard = updated_context.application
              .get(signer_id)
              .map_err(|_| Error::Unauthorized)?;

          // Update application value
          *guard.get_mut() = GuardedValue::Application(application.clone());

          // Only update the context if the operation was successful
          state.contexts.set(context_id.clone(), updated_context);
          Ok(())
      })
  }

  /// Adds new members to the context
  /// # Errors
  /// - `Error::ContextNotFound` - if context doesn't exist
  /// - `Error::Unauthorized` - if signer doesn't have ManageMembers capability
  fn add_members(
      env: &Env,
      signer_id: &BytesN<32>,
      context_id: &BytesN<32>,
      members: &Vec<BytesN<32>>,
  ) -> Result<(), Error> {
      Self::update_state(env, |state| {
          let context = state.contexts
              .get(context_id.clone())
              .ok_or(Error::ContextNotFound)?;

          let mut updated_context = context.clone();
          let mut new_members = Vec::new(env);

          // Scope for guard_ref to release borrow
          {
              let guard_ref = updated_context.members
                  .get(signer_id)
                  .map_err(|_| Error::Unauthorized)?;
              
              let mut members_mut = guard_ref.get_mut();
              if let GuardedValue::Members(ref mut member_list) = members_mut.deref_mut() {
                  for member in members.iter() {
                      if !member_list.contains(member.clone()) {
                          member_list.push_back(member.clone());
                          new_members.push_back(member);
                      }
                  }
              }
          }

          // Initialize nonces for new members
          for member in new_members.iter() {
              updated_context.member_nonces.set(member.clone(), 0);
          }

          state.contexts.set(context_id.clone(), updated_context);
          Ok(())
      })
  }

  /// Removes members from the context
  /// # Errors
  /// - `Error::ContextNotFound` - if context doesn't exist
  /// - `Error::Unauthorized` - if signer doesn't have ManageMembers capability
  fn remove_members(
      env: &Env,
      signer_id: &BytesN<32>,
      context_id: &BytesN<32>,
      members: &Vec<BytesN<32>>,
  ) -> Result<(), Error> {
      Self::update_state(env, |state| {
          let context = state.contexts
              .get(context_id.clone())
              .ok_or(Error::ContextNotFound)?;

          let mut updated_context = context.clone();
          let mut members_to_remove = Vec::new(env);

          {
              let guard_ref = updated_context.members
                  .get(signer_id)
                  .map_err(|_| Error::Unauthorized)?;
              
              let mut members_mut = guard_ref.get_mut();
              if let GuardedValue::Members(ref mut member_list) = members_mut.deref_mut() {
                  for member in members.iter() {
                      if member_list.contains(member.clone()) {
                          if let Some(pos) = member_list.iter().position(|m| m == member) {
                              member_list.remove(pos as u32);
                              members_to_remove.push_back(member.clone());
                          }
                      }
                  }
              }
          }

          // Remove nonces for removed members
          for member in members_to_remove.iter() {
              updated_context.member_nonces.remove(member);
          }

          state.contexts.set(context_id.clone(), updated_context);
          Ok(())
      })
  }

  /// Grants capabilities to members
  /// # Errors
  /// - `Error::ContextNotFound` - if context doesn't exist
  /// - `Error::Unauthorized` - if signer doesn't have required capability
  /// - `Error::NotAMember` - if target identity is not a member
  fn grant(
      env: &Env,
      signer_id: &BytesN<32>,
      context_id: &BytesN<32>,
      capabilities: &Vec<(BytesN<32>, Capability)>,
  ) -> Result<(), Error> {
      Self::update_state(env, |state| {
          let context = state.contexts
              .get(context_id.clone())
              .ok_or(Error::ContextNotFound)?;

          let mut updated_context = context.clone();

          // Verify all identities are members
          {
              let members_guard = updated_context.members
                  .get(signer_id)
                  .map_err(|_| Error::Unauthorized)?;

              if let GuardedValue::Members(ref member_list) = members_guard.deref() {
                  for (identity, _) in capabilities.iter() {
                      if !member_list.contains(identity) {
                          return Err(Error::NotAMember);
                      }
                  }
              }
          }

          // Grant capabilities
          for (identity, capability) in capabilities.iter() {
              match capability {
                  Capability::ManageApplication => {
                      let mut guard = updated_context.application
                          .get(signer_id)
                          .map_err(|_| Error::Unauthorized)?;
                      guard.privileges().grant(&identity);
                  },
                  Capability::ManageMembers => {
                      let mut guard = updated_context.members
                          .get(signer_id)
                          .map_err(|_| Error::Unauthorized)?;
                      guard.privileges().grant(&identity);
                  },
                  Capability::Proxy => {
                      let mut guard = updated_context.proxy
                          .get(signer_id)
                          .map_err(|_| Error::Unauthorized)?;
                      guard.privileges().grant(&identity);
                  },
              }
          }

          state.contexts.set(context_id.clone(), updated_context);
          Ok(())
      })
  }

  /// Revokes capabilities from members
  /// # Errors
  /// - `Error::ContextNotFound` - if context doesn't exist
  /// - `Error::Unauthorized` - if signer doesn't have required capability
  fn revoke(
      env: &Env,
      signer_id: &BytesN<32>,
      context_id: &BytesN<32>,
      capabilities: &Vec<(BytesN<32>, Capability)>,
  ) -> Result<(), Error> {
      Self::update_state(env, |state| {
          let context = state.contexts
              .get(context_id.clone())
              .ok_or(Error::ContextNotFound)?;

          let mut updated_context = context.clone();

          for (identity, capability) in capabilities.iter() {
              match capability {
                  Capability::ManageApplication => {
                      let mut guard = updated_context.application
                          .get(signer_id)
                          .map_err(|_| Error::Unauthorized)?;
                      guard.privileges().revoke(&identity);
                  },
                  Capability::ManageMembers => {
                      let mut guard = updated_context.members
                          .get(signer_id)
                          .map_err(|_| Error::Unauthorized)?;
                      guard.privileges().revoke(&identity);
                  },
                  Capability::Proxy => {
                      let mut guard = updated_context.proxy
                          .get(signer_id)
                          .map_err(|_| Error::Unauthorized)?;
                      guard.privileges().revoke(&identity);
                  },
              }
          }

          state.contexts.set(context_id.clone(), updated_context);
          Ok(())
      })
  }

  /// Updates the proxy contract for a context
  /// # Errors
  /// - `Error::ContextNotFound` - if context doesn't exist
  /// - `Error::Unauthorized` - if signer doesn't have Proxy capability
  /// - `Error::InvalidState` - if proxy data is corrupted
  /// - `Error::ProxyCodeNotSet` - if proxy WASM code is not set
  /// - `Error::ProxyUpgradeFailed` - if proxy upgrade fails
  fn update_proxy_contract(
      env: &Env,
      signer_id: &BytesN<32>,
      context_id: &BytesN<32>,
  ) -> Result<(), Error> {
      Self::update_state(env, |state| {
          let context = state.contexts
              .get(context_id.clone())
              .ok_or(Error::ContextNotFound)?;

          let mut updated_context = context.clone();

          // Get proxy contract address
          let proxy_contract_id = {
              let guard_ref = updated_context.proxy
                  .get(signer_id)
                  .map_err(|_| Error::Unauthorized)?;

              match guard_ref.deref() {
                  GuardedValue::Proxy(proxy_id) => proxy_id.clone(),
                  _ => return Err(Error::InvalidState),
              }
          };

          // Get proxy code
          let proxy_code = state.proxy_code
              .to_option()
              .ok_or(Error::ProxyCodeNotSet)?;

          let contract_address = env.current_contract_address();

          // Attempt to upgrade proxy and check response
          match env.try_invoke_contract::<(), Error>(
              &proxy_contract_id,
              &Symbol::new(env, "upgrade"),
              (proxy_code, contract_address).into_val(env),
          ) {
              Ok(_) => {
                  state.contexts.set(context_id.clone(), updated_context);
                  Ok(())
              },
              Err(_) => Err(Error::ProxyUpgradeFailed),
          }
      })
  }

  /// Deploys a new proxy contract for a context
  /// # Errors
  /// - `Error::ProxyCodeNotSet` - if proxy WASM code is not set
  fn deploy_proxy(env: &Env, context_id: &BytesN<32>) -> Result<Address, Error> {
      let state = Self::get_state(env);
      
      // Get stored WASM hash
      let wasm_hash = state.proxy_code
          .to_option()
          .ok_or(Error::ProxyCodeNotSet)?;

      // Deploy new proxy instance using context_id as salt
      let proxy_address = env.deployer()
          .with_address(env.current_contract_address(), context_id.clone())
          .deploy_v2(wasm_hash, ());

      Ok(proxy_address)
  }
}