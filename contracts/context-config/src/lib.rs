#![allow(unused_results, unused_crate_dependencies)]

use std::collections::BTreeMap;

use ed25519_dalek::VerifyingKey;
use near_sdk::store::{IterableMap, IterableSet};
use near_sdk::{env, near, require, serde_json, AccountId, BorshStorageKey, Timestamp};

mod repr;
mod types;

pub use repr::{Repr, ReprBytes};
pub use types::{
    ApplicationId, ApplicationSource, BlobId, ContextId, ContextIdentity, Guard, SignedPayload,
};

#[derive(Debug)]
#[near(contract_state)]
pub struct ContextConfig {
    contexts: IterableMap<ContextId, Context>,
    config: Config,
}

#[derive(Debug)]
#[near(serializers = [borsh])]
struct Config {
    validity_threshold_ms: Timestamp,
}

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct Context {
    pub application: Guard<Application>,
    pub members: Guard<IterableSet<ContextIdentity>>,
}

#[derive(Debug)]
#[near(serializers = [borsh, json])]
pub struct Application {
    pub id: Repr<ApplicationId>,
    pub blob: Repr<BlobId>,
    pub source: ApplicationSource,
    pub metadata: Box<[u8]>,
}

#[derive(Copy, Clone, Debug, BorshStorageKey)]
#[near(serializers = [borsh])]
pub enum Prefix {
    Contexts,
    Members(ContextId),
    Privileges(PrivilegeScope, ContextId),
}

#[derive(Copy, Clone, Debug)]
#[near(serializers = [borsh])]
pub enum PrivilegeScope {
    Application,
    MemberList,
}

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd)]
#[near(serializers = [json])]
pub enum Capability {
    ManageApplication,
    ManageMembers,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            contexts: IterableMap::new(Prefix::Contexts),
            config: Config {
                validity_threshold_ms: 30_000,
            },
        }
    }
}

#[near(serializers = [json])]
#[derive(Debug)]
pub struct AddContextInput {
    pub context_id: Repr<ContextId>,
    pub application: Application,

    // replay minimization
    pub account_id: AccountId,
    pub timestamp_ms: Timestamp,
}

#[near]
impl ContextConfig {
    pub fn add_context(&mut self) {
        let input = env::input().unwrap_or_default();

        let input: SignedPayload<AddContextInput> =
            serde_json::from_slice(&input).expect("Failed to parse input");

        let AddContextInput {
            context_id,
            application,
            timestamp_ms,
            account_id,
        } = input
            .parse(|i| VerifyingKey::from_bytes(i.context_id.as_bytes()))
            .expect("Failed to parse input");

        require!(
            env::block_timestamp_ms().saturating_sub(timestamp_ms)
                <= self.config.validity_threshold_ms,
            "Request expired"
        );

        require!(account_id == env::signer_account_id(), "Not so fast, buddy");

        let members = IterableSet::new(Prefix::Members(*context_id));

        let context = Context {
            application: Guard::new(
                application,
                Prefix::Privileges(PrivilegeScope::Application, *context_id),
            ),
            members: Guard::new(
                members,
                Prefix::Privileges(PrivilegeScope::MemberList, *context_id),
            ),
        };

        if self.contexts.insert(*context_id, context).is_some() {
            env::panic_str("Context already exists");
        }

        env::log_str(&format!("Context `{}` added", context_id));
    }

    pub fn update_application(&mut self, context_id: Repr<ContextId>, application: Application) {
        let context = self
            .contexts
            .get_mut(&context_id)
            .expect("context does not exist");

        let new_application_id = application.id;

        let old_application = std::mem::replace(
            &mut *context
                .application
                .get_mut()
                .expect("unable to update application"),
            application,
        );

        env::log_str(&format!(
            "Updated application `{}` -> `{}`",
            old_application.id, new_application_id
        ))
    }

    pub fn application(&self, context_id: Repr<ContextId>) -> &Application {
        let context = self
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        &context.application
    }

    pub fn add_members(
        &mut self,
        context_id: Repr<ContextId>,
        members: Vec<Repr<ContextIdentity>>,
    ) {
        let context = self
            .contexts
            .get_mut(&context_id)
            .expect("context does not exist");

        let mut ctx_members = context
            .members
            .get_mut()
            .expect("unable to update member list");

        for member in members {
            env::log_str(&format!(
                "Added `{}` as a member of `{}`",
                member, context_id
            ));

            ctx_members.insert(member.into_inner());
        }
    }

    pub fn remove_members(
        &mut self,
        context_id: Repr<ContextId>,
        members: Vec<Repr<ContextIdentity>>,
    ) {
        let context = self
            .contexts
            .get_mut(&context_id)
            .expect("context does not exist");

        let mut ctx_members = context
            .members
            .get_mut()
            .expect("unable to update member list");

        for member in members {
            env::log_str(&format!(
                "Removed `{}` as a member of `{}`",
                member, context_id
            ));

            ctx_members.remove(&member.into_inner());
        }
    }

    pub fn members(
        &self,
        context_id: Repr<ContextId>,
        offset: usize,
        length: usize,
    ) -> Vec<Repr<ContextIdentity>> {
        let context = self
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        let mut members = Vec::with_capacity(length);

        for member in context.members.iter().skip(offset).take(length) {
            members.push(Repr::new(*member));
        }

        members
    }

    pub fn grant(
        &mut self,
        context_id: Repr<ContextId>,
        capabilities: Vec<(AccountId, Capability)>,
    ) {
        let context = self
            .contexts
            .get_mut(&context_id)
            .expect("context does not exist");

        for (account_id, capability) in capabilities {
            match capability {
                Capability::ManageApplication => context
                    .application
                    .get_mut()
                    .expect("unable to update application")
                    .priviledges()
                    .grant(account_id),
                Capability::ManageMembers => context
                    .members
                    .get_mut()
                    .expect("unable to update member list")
                    .priviledges()
                    .grant(account_id),
            };
        }
    }

    pub fn revoke(
        &mut self,
        context_id: Repr<ContextId>,
        capabilities: Vec<(AccountId, Capability)>,
    ) {
        let context = self
            .contexts
            .get_mut(&context_id)
            .expect("context does not exist");

        for (account_id, capability) in capabilities {
            match capability {
                Capability::ManageApplication => context
                    .application
                    .get_mut()
                    .expect("unable to update application")
                    .priviledges()
                    .revoke(account_id),
                Capability::ManageMembers => context
                    .members
                    .get_mut()
                    .expect("unable to update member list")
                    .priviledges()
                    .revoke(account_id),
            };
        }
    }

    pub fn privileges(
        &self,
        context_id: Repr<ContextId>,
        account_ids: Vec<AccountId>,
    ) -> BTreeMap<AccountId, Vec<Capability>> {
        let context = self
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        let mut privileges = BTreeMap::<_, Vec<_>>::new();

        let application_privileges = context.application.priviledged();
        let member_privileges = context.members.priviledged();

        if account_ids.is_empty() {
            for account_id in application_privileges {
                privileges
                    .entry(account_id.clone())
                    .or_default()
                    .push(Capability::ManageApplication);
            }

            for account_id in member_privileges {
                privileges
                    .entry(account_id.clone())
                    .or_default()
                    .push(Capability::ManageMembers);
            }
        } else {
            for account_id in &account_ids {
                let entry = privileges.entry(account_id.clone()).or_default();

                if application_privileges.contains(account_id) {
                    entry.push(Capability::ManageApplication);
                }

                if member_privileges.contains(account_id) {
                    entry.push(Capability::ManageMembers);
                }
            }
        }

        privileges
    }

    pub fn set_validity_threshold_ms(&mut self, validity_threshold_ms: Timestamp) {
        require!(
            env::signer_account_id() == env::current_account_id(),
            "Unauthorized access"
        );

        self.config.validity_threshold_ms = validity_threshold_ms;
    }
}
