use std::collections::BTreeMap;

use calimero_context_config::repr::ReprTransmute;
use candid::Principal;
use ic_cdk_macros::query;

use crate::types::*;
use crate::CONTEXT_CONFIGS;

#[query]
fn application(context_id: ICContextId) -> ICApplication {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        (*context.application).clone()
    })
}

#[query]
fn application_revision(context_id: ICContextId) -> u64 {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        context.application.revision()
    })
}

#[query]
fn proxy_contract(context_id: ICContextId) -> String {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        (*context.proxy).clone()
    }).to_string()
}

#[query]
fn members(context_id: ICContextId, offset: usize, length: usize) -> Vec<ICContextIdentity> {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        let members = &*context.members;
        members.iter().skip(offset).take(length).cloned().collect()
    })
}

#[query]
fn has_member(context_id: ICContextId, identity: ICContextIdentity) -> bool {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        context.members.contains(&identity)
    })
}

#[query]
fn members_revision(context_id: ICContextId) -> u64 {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        context.members.revision()
    })
}

#[query]
fn privileges(
    context_id: ICContextId,
    identities: Vec<ICContextIdentity>,
) -> BTreeMap<ICSignerId, Vec<ICCapability>> {
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        let mut privileges: BTreeMap<ICSignerId, Vec<ICCapability>> = BTreeMap::new();

        let application_privileges = context.application.privileged();
        let member_privileges = context.members.privileged();

        if identities.is_empty() {
            for signer_id in application_privileges {
                privileges
                    .entry(*signer_id)
                    .or_default()
                    .push(ICCapability::ManageApplication);
            }

            for signer_id in member_privileges {
                privileges
                    .entry(*signer_id)
                    .or_default()
                    .push(ICCapability::ManageMembers);
            }
        } else {
            for identity in identities {
                let entry = privileges
                    .entry(identity.rt().expect("infallible conversion"))
                    .or_default();

                if application_privileges.contains(&identity.rt().expect("infallible conversion")) {
                    entry.push(ICCapability::ManageApplication);
                }
                if member_privileges.contains(&identity.rt().expect("infallible conversion")) {
                    entry.push(ICCapability::ManageMembers);
                }
            }
        }

        privileges
    })
}
