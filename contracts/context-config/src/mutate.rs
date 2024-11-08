use core::mem;

use calimero_context_config::repr::{Repr, ReprBytes, ReprTransmute};
use calimero_context_config::types::{
    Application, Capability, ContextId, ContextIdentity, Signed, SignerId,
};
use calimero_context_config::{ContextRequest, ContextRequestKind, Request, RequestKind};
use near_sdk::serde_json::{self, json};
use near_sdk::store::IterableSet;
use near_sdk::{
    env, near, require, AccountId, Gas, NearToken, Promise, PromiseError, PromiseOrValue,
};

use super::{
    parse_input, Context, ContextConfigs, ContextConfigsExt, ContextPrivilegeScope, Guard, Prefix,
    PrivilegeScope,
};

#[near]
impl ContextConfigs {
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
            RequestKind::Context(ContextRequest {
                context_id, kind, ..
            }) => match kind {
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
                ContextRequestKind::UpdateProxyContract => {
                    self.update_proxy_contract(&request.signer_id, context_id);
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
    ) -> Promise {
        require!(
            signer_id.as_bytes() == context_id.as_bytes(),
            "context addition must be signed by the context itself"
        );

        let mut members = IterableSet::new(Prefix::Members(*context_id));
        let _ = members.insert(*author_id);

        // Create incremental account ID
        let account_id: AccountId = format!("{}.{}", self.next_proxy_id, env::current_account_id())
            .parse()
            .expect("invalid account ID");

        self.next_proxy_id += 1;

        let context = Context {
            application: Guard::new(
                Prefix::Privileges(PrivilegeScope::Context(
                    *context_id,
                    ContextPrivilegeScope::Application,
                )),
                author_id.rt().expect("infallible conversion"),
                Application::new(
                    application.id,
                    application.blob,
                    application.size,
                    application.source.to_owned(),
                    application.metadata.to_owned(),
                ),
            ),
            members: Guard::new(
                Prefix::Privileges(PrivilegeScope::Context(
                    *context_id,
                    ContextPrivilegeScope::MemberList,
                )),
                author_id.rt().expect("infallible conversion"),
                members,
            ),
            proxy: Guard::new(
                Prefix::Privileges(PrivilegeScope::Context(
                    *context_id,
                    ContextPrivilegeScope::Proxy,
                )),
                author_id.rt().expect("infallible conversion"),
                account_id.clone(),
            ),
        };

        if self.contexts.insert(*context_id, context).is_some() {
            env::panic_str("context already exists");
        }

        env::log_str(&format!("Context `{}` added", context_id));

        // Deploy proxy contract
        self.deploy_proxy_contract(context_id, account_id)
            .then(Self::ext(env::current_account_id()).add_context_callback())
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

        let old_application = mem::replace(
            &mut *context
                .application
                .get(signer_id)
                .expect("unable to update application")
                .get_mut(),
            Application::new(
                application.id,
                application.blob,
                application.size,
                application.source.to_owned(),
                application.metadata.to_owned(),
            ),
        );

        env::log_str(&format!(
            "Updated application for context `{}` from `{}` to `{}`",
            context_id, old_application.id, new_application_id
        ));
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
            .get(signer_id)
            .expect("unable to update member list")
            .get_mut();

        for member in members {
            env::log_str(&format!("Added `{member}` as a member of `{context_id}`"));

            let _ = ctx_members.insert(*member);
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
            .get(signer_id)
            .expect("unable to update member list")
            .get_mut();

        for member in members {
            let _ = ctx_members.remove(&member);
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
                    .get(signer_id)
                    .expect("unable to update application")
                    .priviledges()
                    .grant(identity),
                Capability::ManageMembers => context
                    .members
                    .get(signer_id)
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
                    .get(signer_id)
                    .expect("unable to update application")
                    .priviledges()
                    .revoke(&identity),
                Capability::ManageMembers => context
                    .members
                    .get(signer_id)
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

    pub fn deploy_proxy_contract(
        &mut self,
        context_id: Repr<ContextId>,
        account_id: AccountId,
    ) -> Promise {
        // Known constants from NEAR protocol
        //Use near tokens instead of yoctoNEAR
        const ACCOUNT_CREATION_COST: u128 = 1_000_000_000_000_000_000_000; // 0.001 NEAR
        const MIN_ACCOUNT_BALANCE: u128 = 3_500_000_000_000_000_000_000; // 0.0035 NEAR

        // Calculate storage needs
        let contract_bytes = self.proxy_code.get().clone().unwrap();
        let storage_cost = (contract_bytes.len() as u128) * env::storage_byte_cost().as_yoctonear();

        // Calculate required deposit
        let required_deposit = NearToken::from_yoctonear(
            ACCOUNT_CREATION_COST +    // Cost to create account
            MIN_ACCOUNT_BALANCE +      // Minimum balance required
            storage_cost +             // Storage cost for contract
            10_000_000_000_000_000_000, // Additional deposit for storage
        );

        require!(
            env::account_balance() >= required_deposit,
            "Insufficient contract balance for deployment"
        );

        // Calculate init gas dynamically
        let init_args = serde_json::to_vec(&json!({
            "context_id": context_id,
            "context_config_account_id": env::current_account_id()
        }))
        .unwrap();

        let init_gas = Gas::from_gas(
            Gas::from_tgas(20).as_gas()
                + ((init_args.len() as u64) + contract_bytes.len() as u64) * 100_000,
        );

        Promise::new(account_id)
            .create_account()
            .transfer(required_deposit)
            .deploy_contract(contract_bytes)
            .function_call(
                "init".to_owned(),
                init_args,
                NearToken::from_near(0),
                init_gas,
            )
            .then(Self::ext(env::current_account_id()).proxy_deployment_callback())
    }

    fn update_proxy_contract(
        &mut self,
        signer_id: &SignerId,
        context_id: Repr<ContextId>,
    ) -> Promise {
        // Get the context and verify proxy contract exists
        let context = self
            .contexts
            .get_mut(&context_id)
            .expect("context does not exist");

        let proxy_account_id = context
            .proxy
            .get(signer_id)
            .expect("unable to update contract")
            .get_mut();

        let new_code = self.proxy_code.get().clone().unwrap();

        // Call the update method on the proxy contract
        Promise::new(proxy_account_id.clone())
            .function_call(
                "update_contract".to_owned(),
                new_code,
                NearToken::from_near(0),
                Gas::from_tgas(100),
            )
            .then(Self::ext(env::current_account_id()).update_proxy_callback())
    }
}

#[near]
impl ContextConfigs {
    pub fn proxy_deployment_callback(
        &mut self,
        #[callback_result] call_result: Result<(), PromiseError>,
    ) -> PromiseOrValue<()> {
        if let Ok(_) = call_result {
            // Calculate actual storage used and refund excess
            let actual_storage_used = env::storage_usage();
            let actual_cost =
                (actual_storage_used as u128) * env::storage_byte_cost().as_yoctonear();
            let deposit_used = env::attached_deposit().as_yoctonear();

            env::log_str("Successfully deployed proxy contract");

            if actual_cost < deposit_used {
                Promise::new(env::current_account_id())
                    .transfer(NearToken::from_yoctonear(deposit_used - actual_cost))
                    .into()
            } else {
                PromiseOrValue::Value(())
            }
        } else {
            env::panic_str("Failed to deploy proxy contract");
        }
    }

    pub fn add_context_callback(
        &mut self,
        #[callback_result] call_result: Result<(), PromiseError>,
    ) {
        if let Err(e) = call_result {
            panic!("Failed to deploy proxy contract for context: {:?}", e);
        }
    }

    #[private]
    #[handle_result]
    pub fn update_contract_callback(
        &mut self,
        #[callback_result] call_result: Result<(), PromiseError>,
    ) -> Result<(), &'static str> {
        if let Err(e) = call_result {
            panic!("Failed to update proxy contract: {:?}", e);
        }

        Ok(())
    }
}
