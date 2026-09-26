pub mod get_application;
pub mod get_application_abi;
pub mod install_application;
pub mod install_dev_application;
pub mod list_application_versions;
pub mod list_applications;
pub mod uninstall_application;

use calimero_primitives::application::ApplicationId;
use tracing::{info, warn};

use crate::AdminState;

/// Compile a just-installed application's modules before answering the
/// install, so the first context created from it does not pay for compiling
/// its WASM inside that request. A failure is logged, not returned: the
/// application is installed either way, and a module that will not compile now
/// fails the same way when a context first uses it.
async fn precompile(state: &AdminState, application_id: ApplicationId) {
    match state
        .ctx_client
        .precompile_application(
            calimero_context_client::messages::PrecompileApplicationRequest { application_id },
        )
        .await
    {
        Ok(compiled) => info!(%application_id, compiled, "Application modules precompiled"),
        Err(err) => warn!(%application_id, error=?err, "Could not precompile application modules"),
    }
}
