use std::ops::Deref;

use calimero_context_config::icp::repr::ICRepr;
use calimero_context_config::icp::types::{
    ICApplication, ICCapability, ICContextRequest, ICContextRequestKind, ICRequest, ICRequestKind,
    ICSigned,
};
use calimero_context_config::repr::{ReprBytes, ReprTransmute};
use calimero_context_config::types::{ContextId, ContextIdentity, SignerId};
use candid::Principal;
use ic_cdk::api::management_canister::main::{
    create_canister, install_code, CanisterSettings, CreateCanisterArgument, InstallCodeArgument,
};

use crate::guard::Guard;
use crate::{with_state, with_state_mut, Context};

#[ic_cdk::update]
pub async fn mutate(signed_request: ICSigned<ICRequest>) -> Result<(), String> {
    let request = signed_request
        .parse(|r| *r.signer_id)
        .map_err(|e| format!("Failed to verify signature: {}", e))?;

    // Check request timestamp
    let current_time = ic_cdk::api::time();
    if current_time.saturating_sub(request.timestamp_ms) > 1000 * 5 {
        // 5 seconds threshold
        return Err("request expired".to_string());
    }

    match request.kind {
        ICRequestKind::Context(ICContextRequest { context_id, kind }) => match kind {
            ICContextRequestKind::Add {
                author_id,
                application,
            } => add_context(&request.signer_id, context_id, *author_id, application).await,
            ICContextRequestKind::UpdateApplication { application } => {
                update_application(&request.signer_id, &context_id, application)
            }
            ICContextRequestKind::AddMembers { members } => {
                add_members(&request.signer_id, &context_id, members)
            }
            ICContextRequestKind::RemoveMembers { members } => {
                remove_members(&request.signer_id, &context_id, members)
            }
            ICContextRequestKind::Grant { capabilities } => {
                grant(&request.signer_id, &context_id, capabilities)
            }
            ICContextRequestKind::Revoke { capabilities } => {
                revoke(&request.signer_id, &context_id, capabilities)
            }
            ICContextRequestKind::UpdateProxyContract => {
                update_proxy_contract(&request.signer_id, context_id).await
            }
        },
    }
}

async fn add_context(
    signer_id: &SignerId,
    context_id: ICRepr<ContextId>,
    author_id: ContextIdentity,
    application: ICApplication,
) -> Result<(), String> {
    if signer_id.as_bytes() != context_id.as_bytes() {
        return Err("context addition must be signed by the context itself".into());
    }

    let proxy_canister_id = deploy_proxy_contract(context_id)
        .await
        .unwrap_or_else(|e| panic!("Failed to deploy proxy contract: {}", e));

    with_state_mut(|configs| {
        // Create context with guards
        let context = Context {
            application: Guard::new(author_id.rt().expect("infallible conversion"), application),
            members: Guard::new(
                author_id.rt().expect("infallible conversion"),
                [author_id.rt().expect("infallible conversion")].into(),
            ),
            proxy: Guard::new(
                author_id.rt().expect("infallible conversion"),
                proxy_canister_id,
            ),
        };

        // Store context
        if configs.contexts.insert(context_id, context).is_some() {
            return Err("context already exists".into());
        }

        Ok(())
    })
}

async fn deploy_proxy_contract(context_id: ICRepr<ContextId>) -> Result<Principal, String> {
    // Get the proxy code
    let proxy_code =
        with_state(|configs| configs.proxy_code.clone()).ok_or("proxy code not set")?;

    // Get the ledger ID
    let ledger_id = with_state(|configs| configs.ledger_id.clone());
    // Create canister with cycles
    let create_args = CreateCanisterArgument {
        settings: Some(CanisterSettings {
            controllers: Some(vec![ic_cdk::api::id()]),
            compute_allocation: None,
            memory_allocation: None,
            freezing_threshold: None,
            reserved_cycles_limit: None,
            log_visibility: None,
            wasm_memory_limit: None,
        }),
    };

    let (canister_record,) = create_canister(create_args, 500_000_000_000_000u128)
        .await
        .map_err(|e| format!("Failed to create canister: {:?}", e))?;

    let canister_id = canister_record.canister_id;

    // Encode init args matching the proxy's init(context_id: ICRepr<ContextId>, ledger_id: Principal)
    let init_args = candid::encode_args((context_id, ledger_id))
        .map_err(|e| format!("Failed to encode init args: {}", e))?;

    let install_args = InstallCodeArgument {
        mode: ic_cdk::api::management_canister::main::CanisterInstallMode::Install,
        canister_id,
        wasm_module: proxy_code,
        arg: init_args,
    };

    install_code(install_args)
        .await
        .map_err(|e| format!("Failed to install code: {:?}", e))?;

    Ok(canister_id)
}

fn update_application(
    signer_id: &SignerId,
    context_id: &ContextId,
    application: ICApplication,
) -> Result<(), String> {
    with_state_mut(|configs| {
        // Get the context or return error if it doesn't exist
        let context = configs
            .contexts
            .get_mut(context_id)
            .ok_or_else(|| "context does not exist".to_string())?;

        // Get mutable access to the application through the Guard
        let guard_ref = context
            .application
            .get(signer_id)
            .map_err(|e| e.to_string())?;
        let mut app_ref = guard_ref.get_mut();

        // Replace the application with the new one
        *app_ref = application;

        Ok(())
    })
}

fn add_members(
    signer_id: &SignerId,
    context_id: &ContextId,
    members: Vec<ICRepr<ContextIdentity>>,
) -> Result<(), String> {
    with_state_mut(|configs| {
        // Get the context or return error if it doesn't exist
        let context = configs
            .contexts
            .get_mut(context_id)
            .ok_or_else(|| "context does not exist".to_string())?;

        // Get mutable access to the members through the Guard
        let guard_ref = context.members.get(signer_id).map_err(|e| e.to_string())?;
        let mut ctx_members = guard_ref.get_mut();

        // Add each member
        for member in members {
            ctx_members.insert(member);
        }

        Ok(())
    })
}

fn remove_members(
    signer_id: &SignerId,
    context_id: &ContextId,
    members: Vec<ICRepr<ContextIdentity>>,
) -> Result<(), String> {
    with_state_mut(|configs| {
        // Get the context or return error if it doesn't exist
        let context = configs
            .contexts
            .get_mut(context_id)
            .ok_or_else(|| "context does not exist".to_string())?;

        // Get mutable access to the members through the Guard
        let mut ctx_members = context
            .members
            .get(signer_id)
            .map_err(|e| e.to_string())?
            .get_mut();

        for member in members {
            ctx_members.remove(&member);

            // Revoke privileges
            ctx_members
                .privileges()
                .revoke(&member.rt().expect("infallible conversion"));
            context
                .application
                .privileges()
                .revoke(&member.rt().expect("infallible conversion"));
        }

        Ok(())
    })
}

fn grant(
    signer_id: &SignerId,
    context_id: &ContextId,
    capabilities: Vec<(ICRepr<ContextIdentity>, ICCapability)>,
) -> Result<(), String> {
    with_state_mut(|configs| {
        let context = configs
            .contexts
            .get_mut(context_id)
            .ok_or_else(|| "context does not exist".to_string())?;

        for (identity, capability) in capabilities {
            let is_member = context.members.deref().contains(&identity);

            if !is_member {
                return Err("unable to grant privileges to non-member".to_string());
            }

            match capability {
                ICCapability::ManageApplication => {
                    context
                        .application
                        .get(signer_id)
                        .map_err(|e| e.to_string())?
                        .privileges()
                        .grant(identity.rt().expect("infallible conversion"));
                }
                ICCapability::ManageMembers => {
                    context
                        .members
                        .get(signer_id)
                        .map_err(|e| e.to_string())?
                        .privileges()
                        .grant(identity.rt().expect("infallible conversion"));
                }
                ICCapability::Proxy => {
                    context
                        .proxy
                        .get(signer_id)
                        .map_err(|e| e.to_string())?
                        .privileges()
                        .grant(identity.rt().expect("infallible conversion"));
                }
            }
        }

        Ok(())
    })
}

fn revoke(
    signer_id: &SignerId,
    context_id: &ContextId,
    capabilities: Vec<(ICRepr<ContextIdentity>, ICCapability)>,
) -> Result<(), String> {
    with_state_mut(|configs| {
        let context = configs
            .contexts
            .get_mut(context_id)
            .ok_or_else(|| "context does not exist".to_string())?;

        for (identity, capability) in capabilities {
            match capability {
                ICCapability::ManageApplication => {
                    context
                        .application
                        .get(signer_id)
                        .map_err(|e| e.to_string())?
                        .privileges()
                        .revoke(&identity.rt().expect("infallible conversion"));
                }
                ICCapability::ManageMembers => {
                    context
                        .members
                        .get(signer_id)
                        .map_err(|e| e.to_string())?
                        .privileges()
                        .revoke(&identity.rt().expect("infallible conversion"));
                }
                ICCapability::Proxy => {
                    context
                        .proxy
                        .get(signer_id)
                        .map_err(|e| e.to_string())?
                        .privileges()
                        .revoke(&identity.rt().expect("infallible conversion"));
                }
            }
        }

        Ok(())
    })
}

async fn update_proxy_contract(
    signer_id: &SignerId,
    context_id: ICRepr<ContextId>,
) -> Result<(), String> {
    let (proxy_canister_id, proxy_code) = with_state_mut(|configs| {
        let context = configs
            .contexts
            .get_mut(&context_id)
            .ok_or_else(|| "context does not exist".to_string())?;

        let proxy_cannister = *context
            .proxy
            .get(signer_id)
            .map_err(|_| "unauthorized: Proxy capability required".to_string())?
            .get_mut();

        let proxy_code = configs.proxy_code.clone().ok_or("proxy code not set")?;

        Ok::<_, String>((proxy_cannister, proxy_code))
    })?;

    // Update the proxy contract code
    let install_args = InstallCodeArgument {
        mode: ic_cdk::api::management_canister::main::CanisterInstallMode::Upgrade(None),
        canister_id: proxy_canister_id,
        wasm_module: proxy_code,
        arg: candid::encode_one(&context_id).map_err(|e| format!("Encoding error: {}", e))?,
    };

    install_code(install_args)
        .await
        .map_err(|e| format!("Failed to update proxy contract: {:?}", e))?;

    Ok(())
}
