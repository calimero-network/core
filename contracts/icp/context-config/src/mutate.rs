use std::ops::Deref;

use crate::guard::Guard;
use crate::types::{
    Context, ContextRequest, ContextRequestKind, ICApplication, ICCapability, ICContextId,
    ICContextIdentity, ICPSigned, ICSignerId, Request, RequestKind,
};
use crate::CONTEXT_CONFIGS;

#[ic_cdk::update]
pub fn mutate(signed_request: ICPSigned<Request>) -> Result<(), String> {
    let request = signed_request
        .parse(|r| &r.signer_id)
        .map_err(|e| format!("Failed to verify signature: {}", e))?;

    // Check request timestamp
    let current_time = ic_cdk::api::time();
    if current_time.saturating_sub(request.timestamp_ms) > 1000 * 5 {
        // 5 seconds threshold
        return Err("request expired".to_string());
    }

    match &request.kind {
        RequestKind::Context(ContextRequest { context_id, kind }) => match kind {
            ContextRequestKind::Add {
                author_id,
                application,
            } => {
                add_context(
                    &request.signer_id,
                    context_id.clone(),
                    author_id.clone(),
                    application.clone(),
                )?;
                Ok(())
            }
            ContextRequestKind::UpdateApplication { application } => {
                update_application(&request.signer_id, &context_id.clone(), application.clone())?;
                Ok(())
            }
            ContextRequestKind::AddMembers { members } => {
                add_members(&request.signer_id, &context_id.clone(), members.clone())?;
                Ok(())
            }
            ContextRequestKind::RemoveMembers { members } => {
                remove_members(&request.signer_id, &context_id.clone(), members.clone())?;
                Ok(())
            }
            ContextRequestKind::Grant { capabilities } => {
                grant(
                    &request.signer_id,
                    &context_id.clone(),
                    capabilities.clone(),
                )?;
                Ok(())
            }
            ContextRequestKind::Revoke { capabilities } => {
                revoke(
                    &request.signer_id,
                    &context_id.clone(),
                    capabilities.clone(),
                )?;
                Ok(())
            }
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
    if signer_id.0 != context_id.0 {
        return Err("context addition must be signed by the context itself".into());
    }

    CONTEXT_CONFIGS.with(|configs| {
        let mut configs = configs.borrow_mut();

        // Create context with guards
        let context = Context {
            application: Guard::new(author_id.clone(), application),
            members: Guard::new(author_id.clone(), vec![author_id.clone()]),
            proxy: Guard::new(
                author_id,
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

        // Store the old application ID for logging
        let old_application_id = app_ref.id.clone();

        // Replace the application with the new one
        *app_ref = application.clone();

        // Log the update
        ic_cdk::println!(
            "Updated application for context `{:?}` from `{:?}` to `{:?}`",
            context_id,
            old_application_id,
            application.id
        );

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
            ic_cdk::println!("Added `{:?}` as a member of `{:?}`", member, context_id);

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

            // Log the removal
            ic_cdk::println!(
                "Removed `{:?}` from being a member of `{:?}`",
                member,
                context_id
            );

            // Revoke privileges
            ctx_members.privileges().revoke(&member);
            context.application.privileges().revoke(&member);
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
                        .grant(identity.clone());
                }
                ICCapability::ManageMembers => {
                    context
                        .members
                        .get(signer_id)
                        .map_err(|e| e.to_string())?
                        .privileges()
                        .grant(identity.clone());
                }
            }

            ic_cdk::println!(
                "Granted `{:?}` to `{:?}` in `{:?}`",
                capability,
                identity,
                context_id
            );
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
                        .revoke(&identity);
                }
                ICCapability::ManageMembers => {
                    context
                        .members
                        .get(signer_id)
                        .map_err(|e| e.to_string())?
                        .privileges()
                        .revoke(&identity);
                }
            }

            ic_cdk::println!(
                "Revoked `{:?}` from `{:?}` in `{:?}`",
                capability,
                identity,
                context_id
            );
        }

        Ok(())
    })
}
