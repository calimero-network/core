use crate::guard::Guard;
use crate::types::{Context, ContextRequest, ContextRequestKind, ICApplication, ICContextId, ICContextIdentity, ICPSigned, ICSignerId, Request, RequestKind};
use crate::CONTEXT_CONFIGS;

#[ic_cdk::update]
pub fn mutate(signed_request: ICPSigned<Request>) -> Result<(), String> {
    let request = signed_request.parse(|r| r.signer_id)
        .map_err(|e| format!("Failed to verify signature: {}", e))?;

    // Check request timestamp
    let current_time = ic_cdk::api::time();
    if current_time.saturating_sub(request.timestamp_ms) > 1000 * 60 * 5 { // 5 minutes threshold
        return Err("request expired".to_string());
    }

    match &request.kind {
        RequestKind::Context(ContextRequest {
            context_id,
            kind,
        }) => match kind {
            ContextRequestKind::Add {
                author_id,
                application,
            } => {
                add_context(&request.signer_id, context_id.clone(), author_id.clone(), application.clone())?;
                Ok(())
            }
            ContextRequestKind::UpdateApplication { application } => {
                // TODO: Implement update_application
                Ok(())
            }
            ContextRequestKind::AddMembers { members } => {
                // TODO: Implement add_members
                Ok(())
            }
            ContextRequestKind::RemoveMembers { members } => {
                // TODO: Implement remove_members
                Ok(())
            }
            ContextRequestKind::Grant { capabilities } => {
                // TODO: Implement grant
                Ok(())
            }
            ContextRequestKind::Revoke { capabilities } => {
                // TODO: Implement revoke
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
                format!("{}.{}", configs.next_proxy_id, ic_cdk::api::id())
            ),
        };

        // Store context
        if configs.contexts.insert(context_id.clone(), context).is_some() {
            return Err("context already exists".into());
        }

        configs.next_proxy_id += 1;
        
        Ok(())
    })
}