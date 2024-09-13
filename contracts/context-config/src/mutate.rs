use std::time;

use calimero_context_config::repr::{Repr, ReprBytes, ReprTransmute};
use calimero_context_config::types::{
    Application, Capability, ContextId, ContextIdentity, Signed, SignerId,
};
use calimero_context_config::{
    ContextRequest, ContextRequestKind, Request, RequestKind, SystemRequest, Timestamp,
};
use near_sdk::store::IterableSet;
use near_sdk::{env, near, require, serde_json};

use super::{
    Context, ContextConfigs, ContextConfigsExt, ContextPrivilegeScope, Guard, Prefix,
    PrivilegeScope,
};

const MIN_VALIDITY_THRESHOLD_MS: Timestamp = 5_000;

macro_rules! parse_input {
    ($input:ident $(: $input_ty:ty)?) => {
        let $input = env::input().unwrap_or_default();

        let $input $(: $input_ty )? = serde_json::from_slice(&$input).expect("failed to parse input");
    };
}

#[near]
impl ContextConfigs {
    pub fn set(&mut self) {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "access denied"
        );

        parse_input!(request);

        match request {
            SystemRequest::SetValidityThreshold { threshold_ms } => {
                self.set_validity_threshold_ms(threshold_ms);
            }
        }
    }

    pub fn mutate(&mut self) {
        parse_input!(request: Signed<Request<'_>>);

        let request = request
            .parse(|i| *i.signer_id)
            .expect("failed to parse input");

        require!(
            env::block_timestamp_ms().saturating_sub(request.timestamp_ms)
                <= self.config.validity_threshold_ms,
            "request expired"
        );

        match request.kind {
            RequestKind::Context(ContextRequest { context_id, kind }) => match kind {
                ContextRequestKind::Add {
                    author_id,
                    application,
                } => {
                    self.add_context(&request.signer_id, context_id, author_id, application);
                }
                ContextRequestKind::UpdateApplication { application } => {
                    self.update_application(&request.signer_id, context_id, application);
                }
                ContextRequestKind::AddMembers { members } => {
                    self.add_members(&request.signer_id, context_id, members.into_owned());
                }
                ContextRequestKind::RemoveMembers { members } => {
                    self.remove_members(&request.signer_id, context_id, members.into_owned());
                }
                ContextRequestKind::Grant { capabilities } => {
                    self.grant(&request.signer_id, context_id, capabilities.into_owned());
                }
                ContextRequestKind::Revoke { capabilities } => {
                    self.revoke(&request.signer_id, context_id, capabilities.into_owned());
                }
            },
        }
    }
}

impl ContextConfigs {
    fn add_context(
        &mut self,
        signer_id: &SignerId,
        context_id: Repr<ContextId>,
        author_id: Repr<ContextIdentity>,
        application: Application<'_>,
    ) {
        require!(
            signer_id.as_bytes() == context_id.as_bytes(),
            "context addition must be signed by the context itself"
        );

        let mut members = IterableSet::new(Prefix::Members(*context_id));

        members.insert(*author_id);

        let context = Context {
            application: Guard::new(
                Prefix::Privileges(PrivilegeScope::Context(
                    *context_id,
                    ContextPrivilegeScope::Application,
                )),
                author_id.rt().expect("infallible conversion"),
                Application {
                    id: application.id,
                    blob: application.blob,
                    source: application.source.to_owned(),
                    metadata: application.metadata.to_owned(),
                },
            ),
            members: Guard::new(
                Prefix::Privileges(PrivilegeScope::Context(
                    *context_id,
                    ContextPrivilegeScope::MemberList,
                )),
                author_id.rt().expect("infallible conversion"),
                members,
            ),
        };

        if self.contexts.insert(*context_id, context).is_some() {
            env::panic_str("context already exists");
        }

        env::log_str(&format!("Context `{}` added", context_id));
    }

    fn update_application(
        &mut self,
        signer_id: &SignerId,
        context_id: Repr<ContextId>,
        application: Application<'_>,
    ) {
        let context = self
            .contexts
            .get_mut(&context_id)
            .expect("context does not exist");

        let new_application_id = application.id;

        let old_application = std::mem::replace(
            &mut *context
                .application
                .get_mut(signer_id)
                .expect("unable to update application"),
            Application {
                id: application.id,
                blob: application.blob,
                source: application.source.to_owned(),
                metadata: application.metadata.to_owned(),
            },
        );

        env::log_str(&format!(
            "Updated application for context `{}` from `{}` to `{}`",
            context_id, old_application.id, new_application_id
        ))
    }

    fn add_members(
        &mut self,
        signer_id: &SignerId,
        context_id: Repr<ContextId>,
        members: Vec<Repr<ContextIdentity>>,
    ) {
        let context = self
            .contexts
            .get_mut(&context_id)
            .expect("context does not exist");

        let mut ctx_members = context
            .members
            .get_mut(signer_id)
            .expect("unable to update member list");

        for member in members {
            env::log_str(&format!(
                "Added `{}` as a member of `{}`",
                member, context_id
            ));

            ctx_members.insert(*member);
        }
    }

    fn remove_members(
        &mut self,
        signer_id: &SignerId,
        context_id: Repr<ContextId>,
        members: Vec<Repr<ContextIdentity>>,
    ) {
        let context = self
            .contexts
            .get_mut(&context_id)
            .expect("context does not exist");

        let mut ctx_members = context
            .members
            .get_mut(signer_id)
            .expect("unable to update member list");

        for member in members {
            ctx_members.remove(&member);

            let member = member.rt().expect("infallible conversion");

            env::log_str(&format!(
                "Removed `{}` from being a member of `{}`",
                Repr::new(member),
                context_id
            ));

            ctx_members.priviledges().revoke(&member);
            context.application.priviledges().revoke(&member);
        }
    }

    fn grant(
        &mut self,
        signer_id: &SignerId,
        context_id: Repr<ContextId>,
        capabilities: Vec<(Repr<ContextIdentity>, Capability)>,
    ) {
        let context = self
            .contexts
            .get_mut(&context_id)
            .expect("context does not exist");

        for (identity, capability) in capabilities {
            require!(
                context.members.contains(&*identity),
                "unable to grant privileges to non-member"
            );

            let identity = identity.rt().expect("infallible conversion");

            match capability {
                Capability::ManageApplication => context
                    .application
                    .get_mut(signer_id)
                    .expect("unable to update application")
                    .priviledges()
                    .grant(identity),
                Capability::ManageMembers => context
                    .members
                    .get_mut(signer_id)
                    .expect("unable to update member list")
                    .priviledges()
                    .grant(identity),
            };

            env::log_str(&format!(
                "Granted `{:?}` to `{}` in `{}`",
                capability,
                Repr::new(identity),
                context_id
            ));
        }
    }

    fn revoke(
        &mut self,
        signer_id: &SignerId,
        context_id: Repr<ContextId>,
        capabilities: Vec<(Repr<ContextIdentity>, Capability)>,
    ) {
        let context = self
            .contexts
            .get_mut(&context_id)
            .expect("context does not exist");

        for (identity, capability) in capabilities {
            let identity = identity.rt().expect("infallible conversion");

            match capability {
                Capability::ManageApplication => context
                    .application
                    .get_mut(signer_id)
                    .expect("unable to update application")
                    .priviledges()
                    .revoke(&identity),
                Capability::ManageMembers => context
                    .members
                    .get_mut(signer_id)
                    .expect("unable to update member list")
                    .priviledges()
                    .revoke(&identity),
            };

            env::log_str(&format!(
                "Revoked `{:?}` from `{}` in `{}`",
                capability,
                Repr::new(identity),
                context_id
            ));
        }
    }

    fn set_validity_threshold_ms(&mut self, validity_threshold_ms: Timestamp) {
        if validity_threshold_ms < MIN_VALIDITY_THRESHOLD_MS {
            env::panic_str("invalid validity threshold");
        }

        self.config.validity_threshold_ms = validity_threshold_ms;

        env::log_str(&format!(
            "Set validity threshold to `{:?}`",
            time::Duration::from_millis(validity_threshold_ms)
        ));
    }
}
