use std::collections::BTreeMap;

use calimero_context_config::icp::repr::ICRepr;
use calimero_context_config::icp::types::{ICApplication, ICCapability};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_config::types::{ContextId, ContextIdentity, SignerId};
use candid::Principal;
use ic_cdk_macros::query;

use crate::with_state;

#[query]
fn application(context_id: ICRepr<ContextId>) -> ICApplication {
    with_state(|configs| {
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        context.application.clone()
    })
}

#[query]
fn application_revision(context_id: ICRepr<ContextId>) -> u64 {
    with_state(|configs| {
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        context.application.revision()
    })
}

#[query]
fn proxy_contract(context_id: ICRepr<ContextId>) -> Principal {
    with_state(|configs| {
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        context.proxy.clone()
    })
}

#[query]
fn members(
    context_id: ICRepr<ContextId>,
    offset: usize,
    length: usize,
) -> Vec<ICRepr<ContextIdentity>> {
    with_state(|configs| {
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        let members = &*context.members;
        members.iter().skip(offset).take(length).cloned().collect()
    })
}

#[query]
fn has_member(context_id: ICRepr<ContextId>, identity: ICRepr<ContextIdentity>) -> bool {
    with_state(|configs| {
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        context.members.contains(&identity)
    })
}

#[query]
fn members_revision(context_id: ICRepr<ContextId>) -> u64 {
    with_state(|configs| {
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        context.members.revision()
    })
}

#[query]
fn privileges(
    context_id: ICRepr<ContextId>,
    identities: Vec<ICRepr<ContextIdentity>>,
) -> BTreeMap<ICRepr<SignerId>, Vec<ICCapability>> {
    with_state(|configs| {
        let context = configs
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        let mut privileges: BTreeMap<ICRepr<SignerId>, Vec<ICCapability>> = BTreeMap::new();

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
                let signer_id = identity.rt().expect("infallible conversion");

                let entry = privileges.entry(signer_id).or_default();

                if application_privileges.contains(&signer_id) {
                    entry.push(ICCapability::ManageApplication);
                }

                if member_privileges.contains(&signer_id) {
                    entry.push(ICCapability::ManageMembers);
                }
            }
        }

        privileges
    })
}
