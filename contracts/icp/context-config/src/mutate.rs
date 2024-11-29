use std::ops::Deref;

use calimero_context_config::repr::{ReprBytes, ReprTransmute};

use crate::guard::Guard;
use crate::types::{
    ContextRequest, ContextRequestKind, ICApplication, ICCapability, ICContextId,
    ICContextIdentity, ICPSigned, ICSignerId, Request, RequestKind,
};
use crate::{Context, CONTEXT_CONFIGS};

#[ic_cdk::update]
pub fn mutate(signed_request: ICPSigned<Request>) -> Result<(), String> {
    let request = signed_request
        .parse(|r| r.signer_id)
        .map_err(|e| format!("Failed to verify signature: {}", e))?;

    // Check request timestamp
    let current_time = ic_cdk::api::time();
    if current_time.saturating_sub(request.timestamp_ms) > 1000 * 5 {
        // 5 seconds threshold
        return Err("request expired".to_string());
    }

    match request.kind {
        RequestKind::Context(ContextRequest { context_id, kind }) => match kind {
            ContextRequestKind::Add {
                author_id,
                application,
            } => add_context(&request.signer_id, context_id, author_id, application),
            ContextRequestKind::UpdateApplication { application } => {
                update_application(&request.signer_id, &context_id.clone(), application.clone())
            }
            ContextRequestKind::AddMembers { members } => {
                add_members(&request.signer_id, &context_id.clone(), members.clone())
            }
            ContextRequestKind::RemoveMembers { members } => {
                remove_members(&request.signer_id, &context_id.clone(), members.clone())
            }
            ContextRequestKind::Grant { capabilities } => grant(
                &request.signer_id,
                &context_id.clone(),
                capabilities.clone(),
            ),
            ContextRequestKind::Revoke { capabilities } => revoke(
                &request.signer_id,
                &context_id.clone(),
                capabilities.clone(),
            ),
            ContextRequestKind::UpdateProxyContract => {
                // TODO: Implement update_proxy_contract
                Ok(())
            }
        },
    }
}

fn add_context(
    signer_id: &ICSignerId,
    context_id: ICContextId,
    author_id: ICContextIdentity,
    application: ICApplication,
) -> Result<(), String> {
    // 1. Verify signer is the context itself - direct array comparison
    if signer_id.as_bytes() != context_id.as_bytes() {
        return Err("context addition must be signed by the context itself".into());
    }

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
                format!("{}.{}", configs.next_proxy_id, ic_cdk::api::id()),
            ),
        };

        // Store context
        if configs
            .contexts
            .insert(context_id.clone(), context)
            .is_some()
        {
            return Err("context already exists".into());
        }

        configs.next_proxy_id += 1;

        Ok(())
    })
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
        *app_ref = application.clone();

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
