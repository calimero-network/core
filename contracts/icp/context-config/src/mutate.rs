use std::ops::Deref;

use calimero_context_config::repr::{ReprBytes, ReprTransmute};
use candid::Principal;
use ic_cdk::api::management_canister::main::{
    create_canister, install_code, CanisterSettings, CreateCanisterArgument, InstallCodeArgument,
};

use crate::guard::Guard;
use crate::types::{
    ContextRequest, ContextRequestKind, ICApplication, ICCapability, ICContextId,
    ICContextIdentity, ICPSigned, ICSignerId, Request, RequestKind,
};
use crate::{Context, CONTEXT_CONFIGS};

#[ic_cdk::update]
pub async fn mutate(signed_request: ICPSigned<Request>) -> Result<(), String> {
    let request = signed_request
        .parse(|r| r.signer_id)
        .map_err(|e| format!("Failed to verify signature: {}", e))?;

    // Add debug logging
    let current_time = ic_cdk::api::time() / 1_000_000;
    let time_diff = current_time.saturating_sub(request.timestamp_ms);
    if time_diff > 1000 * 5 {
        return Err(format!(
            "request expired: diff={}ms, current={}, request={}",
            time_diff, current_time, request.timestamp_ms
        ));
    }

    match request.kind {
        RequestKind::Context(ContextRequest { context_id, kind }) => match kind {
            ContextRequestKind::Add {
                author_id,
                application,
            } => add_context(&request.signer_id, context_id, author_id, application).await,
            ContextRequestKind::UpdateApplication { application } => {
                update_application(&request.signer_id, &context_id, application)
            }
            ContextRequestKind::AddMembers { members } => {
                add_members(&request.signer_id, &context_id, members)
            }
            ContextRequestKind::RemoveMembers { members } => {
                remove_members(&request.signer_id, &context_id, members)
            }
            ContextRequestKind::Grant { capabilities } => {
                grant(&request.signer_id, &context_id, capabilities)
            }
            ContextRequestKind::Revoke { capabilities } => {
                revoke(&request.signer_id, &context_id, capabilities)
            }
            ContextRequestKind::UpdateProxyContract => {
                update_proxy_contract(&request.signer_id, context_id).await
            }
        },
    }
}

async fn add_context(
    signer_id: &ICSignerId,
    context_id: ICContextId,
    author_id: ICContextIdentity,
    application: ICApplication,
) -> Result<(), String> {
    if signer_id.as_bytes() != context_id.as_bytes() {
        return Err("context addition must be signed by the context itself".into());
    }

    let proxy_canister_id = deploy_proxy_contract(&context_id)
        .await
        .unwrap_or_else(|e| panic!("Failed to deploy proxy contract: {}", e));

    CONTEXT_CONFIGS.with(|configs| {
        let mut configs = configs.borrow_mut();

        // Create context with guards
        let context = Context {
            application: Guard::new(author_id.rt().expect("infallible conversion"), application),
            members: Guard::new(
                author_id.rt().expect("infallible conversion"),
                vec![author_id.rt().expect("infallible conversion")],
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

async fn deploy_proxy_contract(context_id: &ICContextId) -> Result<Principal, String> {
    // Get the proxy code
    let proxy_code = CONTEXT_CONFIGS
        .with(|configs| configs.borrow().proxy_code.clone())
        .ok_or("proxy code not set")?;

    // Get the ledger ID
    let ledger_id = CONTEXT_CONFIGS.with(|configs| configs.borrow().ledger_id.clone());
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

    // Encode init args matching the proxy's init(context_id: ICContextId, ledger_id: Principal)
    let init_args = candid::encode_args((context_id.clone(), ledger_id))
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
    signer_id: &ICSignerId,
    context_id: &ICContextId,
    application: ICApplication,
) -> Result<(), String> {
    CONTEXT_CONFIGS.with(|configs| {
        let mut configs = configs.borrow_mut();

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
    signer_id: &ICSignerId,
    context_id: &ICContextId,
    members: Vec<ICContextIdentity>,
) -> Result<(), String> {
    CONTEXT_CONFIGS.with(|configs| {
        let mut configs = configs.borrow_mut();

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
            ctx_members.push(member);
        }

        Ok(())
    })
}

fn remove_members(
    signer_id: &ICSignerId,
    context_id: &ICContextId,
    members: Vec<ICContextIdentity>,
) -> Result<(), String> {
    CONTEXT_CONFIGS.with(|configs| {
        let mut configs = configs.borrow_mut();

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
            // Remove member from the list
            if let Some(pos) = ctx_members.iter().position(|x| x == &member) {
                ctx_members.remove(pos);
            }

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
    signer_id: &ICSignerId,
    context_id: &ICContextId,
    capabilities: Vec<(ICContextIdentity, ICCapability)>,
) -> Result<(), String> {
    CONTEXT_CONFIGS.with(|configs| {
        let mut configs = configs.borrow_mut();

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
    signer_id: &ICSignerId,
    context_id: &ICContextId,
    capabilities: Vec<(ICContextIdentity, ICCapability)>,
) -> Result<(), String> {
    CONTEXT_CONFIGS.with(|configs| {
        let mut configs = configs.borrow_mut();

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
    signer_id: &ICSignerId,
    context_id: ICContextId,
) -> Result<(), String> {
    let mut context = CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        configs
            .contexts
            .get(&context_id)
            .ok_or_else(|| "context does not exist".to_string())
            .cloned()
    })?;

    // Get proxy canister ID
    let proxy_canister_id = context
        .proxy
        .get(signer_id)
        .map_err(|_| "unauthorized: Proxy capability required".to_string())?
        .get_mut()
        .clone();

    // Get the proxy code
    let proxy_code = CONTEXT_CONFIGS
        .with(|configs| configs.borrow().proxy_code.clone())
        .ok_or("proxy code not set")?;

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
