pub mod get_application;
pub mod get_application_abi;
pub mod install_application;
pub mod install_dev_application;
pub mod list_application_versions;
pub mod list_applications;
pub mod uninstall_application;

use std::sync::Arc;

use calimero_primitives::application::ApplicationId;
use tracing::{info, warn};

use crate::AdminState;

/// Start compiling a just-installed application's modules in the background,
/// so the first context created from it does not have to compile its WASM
/// inside that request; one that arrives before the compile is done waits for
/// the rest of it rather than compiling again. The install answers without
/// waiting: a compile takes seconds in a debug build, and a client that gave
/// up on a slow install would only have moved the timeout. A failure is logged:
/// a module that will not compile now fails the same way when a context first
/// uses it.
fn precompile(state: &Arc<AdminState>, application_id: ApplicationId) {
    let ctx_client = state.ctx_client.clone();
    drop(tokio::spawn(async move {
        match ctx_client
            .precompile_application(
                calimero_context_client::messages::PrecompileApplicationRequest { application_id },
            )
            .await
        {
            Ok(compiled) => info!(%application_id, compiled, "Application modules precompiled"),
            Err(err) => {
                warn!(%application_id, error=?err, "Could not precompile application modules");
            }
        }
    }));
}
