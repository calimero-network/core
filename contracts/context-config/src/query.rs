#![allow(
    clippy::multiple_inherent_impl,
    reason = "Needed to separate NEAR functionality"
)]

use std::collections::BTreeMap;

use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{
    Application, Capability, ContextId, ContextIdentity, Revision, SignerId,
};
use near_sdk::near;

use super::{ContextConfigs, ContextConfigsExt};

#[near]
impl ContextConfigs {
    pub fn application(&self, context_id: Repr<ContextId>) -> &Application<'_> {
        let context = self
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        &context.application
    }

    pub fn application_revision(&self, context_id: Repr<ContextId>) -> Revision {
        let context = self
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        context.application.revision()
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

    pub fn members_revision(&self, context_id: Repr<ContextId>) -> Revision {
        let context = self
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        context.members.revision()
    }

    pub fn privileges(
        &self,
        context_id: Repr<ContextId>,
        identities: Vec<Repr<ContextIdentity>>,
    ) -> BTreeMap<Repr<SignerId>, Vec<Capability>> {
        let context = self
            .contexts
            .get(&context_id)
            .expect("context does not exist");

        let mut privileges = BTreeMap::<_, Vec<_>>::new();

        let application_privileges = context.application.priviledged();
        let member_privileges = context.members.priviledged();

        if identities.is_empty() {
            for signer_id in application_privileges {
                privileges
                    .entry(Repr::new(*signer_id))
                    .or_default()
                    .push(Capability::ManageApplication);
            }

            for signer_id in member_privileges {
                privileges
                    .entry(Repr::new(*signer_id))
                    .or_default()
                    .push(Capability::ManageMembers);
            }
        } else {
            for identity in identities {
                let signer_id = identity.rt().expect("infallible conversion");

                let entry = privileges.entry(signer_id).or_default();

                if application_privileges.contains(&signer_id) {
                    entry.push(Capability::ManageApplication);
                }

                if member_privileges.contains(&signer_id) {
                    entry.push(Capability::ManageMembers);
                }
            }
        }

        privileges
    }
}
