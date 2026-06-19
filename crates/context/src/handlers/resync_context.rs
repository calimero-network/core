use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{ResyncContextRequest, ResyncContextResponse};
use calimero_governance_store::get_group_for_context;
use calimero_store::key;
use eyre::{bail, WrapErr};
use tracing::{info, warn};

use crate::ContextManager;

/// Whether an operator-requested resync may proceed. Group-only (recovery is
/// defined over a group's upgrade ladder), and destructive: a snapshot resync
/// discards local DAG heads, so it refuses when any exist unless `force`.
fn resync_admission(in_group: bool, dag_head_count: usize, force: bool) -> Result<(), String> {
    if !in_group {
        return Err("not in a group; resync recovery is group-only".to_owned());
    }
    if dag_head_count > 0 && !force {
        return Err(format!(
            "holds {dag_head_count} local DAG head(s) that a resync would discard; \
             pass force=true to resync from a peer anyway"
        ));
    }
    Ok(())
}

impl Handler<ResyncContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <ResyncContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        ResyncContextRequest { context_id, force }: ResyncContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        ActorResponse::r#async(
            async move {
                let in_group = get_group_for_context(&datastore, &context_id)?.is_some();
                let heads = datastore
                    .handle()
                    .get(&key::ContextMeta::new(context_id))?
                    .map_or(0, |m| m.dag_heads.len());
                if let Err(refusal) = resync_admission(in_group, heads, force) {
                    bail!("context {context_id}: {refusal}");
                }

                let marker = key::ContextResyncRequested::new(context_id);
                datastore.handle().put(&marker, &())?;
                // Roll the marker back if the sync can't even be enqueued, so a
                // failed request can't leave the context stuck forcing snapshots.
                if let Err(err) = node_client.sync(Some(&context_id), None).await {
                    if let Err(del) = datastore.handle().delete(&marker) {
                        warn!(%context_id, %del, "failed to roll back resync marker");
                    }
                    return Err(err).wrap_err("failed to trigger resync");
                }
                info!(%context_id, force, "context resync requested");
                Ok(ResyncContextResponse {
                    context_id,
                    resync_started: true,
                })
            }
            .into_actor(self),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::resync_admission;

    #[test]
    fn rejects_non_group_context() {
        let err = resync_admission(false, 0, true).unwrap_err();
        assert!(err.contains("group-only"), "got: {err}");
    }

    #[test]
    fn refuses_local_heads_without_force() {
        let err = resync_admission(true, 3, false).unwrap_err();
        assert!(err.contains("3 local DAG head"), "got: {err}");
        assert!(err.contains("force=true"), "got: {err}");
    }

    #[test]
    fn force_overrides_local_heads() {
        assert!(resync_admission(true, 3, true).is_ok());
    }

    #[test]
    fn no_heads_needs_no_force() {
        assert!(resync_admission(true, 0, false).is_ok());
    }
}
