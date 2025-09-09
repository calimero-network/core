use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    InstallDevApplicationRequest, UpdateContextApplicationRequest,
};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{OptionExt, Result};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::cli::Environment;
use crate::output::{ErrorLine, InfoLine};

pub const EXAMPLES: &str = r#"
  # Watch WASM file and update all contexts using this application
  $ meroctl app watch --path ./my-app.wasm

  # Watch with custom metadata
  $ meroctl app watch --path ./my-app.wasm --metadata '{"version": "1.0.0"}'

  # Watch and update contexts based on current application blob  
  $ meroctl app watch --path ./my-app.wasm --current-app-id <app_id>
"#;

#[derive(Debug, Parser)]
#[command(after_help = EXAMPLES)]
#[command(about = "Watch WASM file and update all contexts using the application")]
pub struct WatchCommand {
    /// Path to the WASM file to watch
    #[arg(long, short = 'p', help = "Path to the WASM file to watch for changes")]
    pub path: Utf8PathBuf,

    /// Metadata for the application
    #[arg(long, help = "Metadata needed for the application installation")]
    pub metadata: Option<String>,

    /// Current application ID to find contexts (if not provided, will install and find by blob comparison)
    #[arg(
        long,
        help = "Current application ID - will find all contexts using this app"
    )]
    pub current_app_id: Option<ApplicationId>,
}

impl WatchCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        // First, install the initial application to get the baseline
        let metadata = self.metadata.clone().map(String::into_bytes);

        environment.output.write(&InfoLine(&format!(
            "Installing initial application from {} to establish baseline...",
            self.path
        )));

        let initial_app_id = client
            .install_dev_application(InstallDevApplicationRequest::new(
                self.path.clone(),
                metadata.clone().unwrap_or_default(),
            ))
            .await?
            .data
            .application_id;

        // Determine which application ID to track
        let target_app_id = self.current_app_id.unwrap_or(initial_app_id);

        // Get all contexts that use this application
        let contexts_response = client.list_contexts().await?;
        let target_contexts: Vec<_> = contexts_response
            .data
            .contexts
            .into_iter()
            .filter(|context| context.application_id == target_app_id)
            .map(|context| context.id)
            .collect();

        if target_contexts.is_empty() {
            environment.output.write(&InfoLine(&format!(
                "No contexts found using application {}. You may need to manually update contexts to use this application first.", 
                target_app_id
            )));
            environment.output.write(&InfoLine(
                "The command will still watch for file changes and install new versions.",
            ));
        } else {
            environment.output.write(&InfoLine(&format!(
                "Found {} context(s) using application {}: {:?}",
                target_contexts.len(),
                target_app_id,
                target_contexts
            )));
        }

        // Start watching the file
        watch_app_and_update_contexts(
            environment,
            target_contexts,
            target_app_id,
            self.path,
            metadata,
        )
        .await
    }
}

async fn watch_app_and_update_contexts(
    environment: &mut Environment,
    mut target_contexts: Vec<calimero_primitives::context::ContextId>,
    mut baseline_app_id: ApplicationId,
    path: Utf8PathBuf,
    metadata: Option<Vec<u8>>,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel(1);

    let handle = Handle::current();
    let mut watcher = notify::recommended_watcher(move |evt| {
        handle.block_on(async {
            drop(tx.send(evt).await);
        });
    })?;

    watcher.watch(path.as_std_path(), RecursiveMode::NonRecursive)?;

    environment
        .output
        .write(&InfoLine(&format!("Watching for changes to {path}")));

    while let Some(event) = rx.recv().await {
        let event = match event {
            Ok(event) => event,
            Err(err) => {
                environment.output.write(&ErrorLine(&format!("{err:?}")));
                continue;
            }
        };

        match event.kind {
            EventKind::Modify(ModifyKind::Data(_)) => {}
            EventKind::Remove(_) => {
                environment
                    .output
                    .write(&ErrorLine("File removed, ignoring.."));
                continue;
            }
            EventKind::Any
            | EventKind::Access(_)
            | EventKind::Create(_)
            | EventKind::Modify(_)
            | EventKind::Other => continue,
        }

        // Install the new application version
        let client = environment.client()?;
        let new_application_id = client
            .install_dev_application(InstallDevApplicationRequest::new(
                path.clone(),
                metadata.clone().unwrap_or_default(),
            ))
            .await?
            .data
            .application_id;

        environment.output.write(&InfoLine(&format!(
            "📦 Installed new application version: {}",
            new_application_id
        )));

        // Refresh context list to catch any new contexts using the baseline app
        let contexts_response = client.list_contexts().await?;
        target_contexts = contexts_response
            .data
            .contexts
            .into_iter()
            .filter(|context| context.application_id == baseline_app_id)
            .map(|context| context.id)
            .collect();

        if target_contexts.is_empty() {
            environment.output.write(&InfoLine(&format!(
                "No contexts currently use application {}. Skipping updates.",
                baseline_app_id
            )));
            continue;
        }

        environment.output.write(&InfoLine(&format!(
            "🔄 Updating {} context(s) from {} to {}...",
            target_contexts.len(),
            baseline_app_id,
            new_application_id
        )));

        // Update all contexts with the new application
        let mut success_count = 0;
        let mut error_count = 0;

        for context_id in &target_contexts {
            // Try to get any available identity for this context
            let identities_result = client
                .get_context_identities(context_id, true) // owned = true to get identities we control
                .await;

            let executor_id = match identities_result {
                Ok(identities_response) => {
                    if let Some(first_identity) = identities_response.data.identities.first() {
                        *first_identity
                    } else {
                        environment.output.write(&ErrorLine(&format!(
                            "✗ No identities found for context {}. Skipping.", 
                            context_id
                        )));
                        error_count += 1;
                        continue;
                    }
                }
                Err(err) => {
                    environment.output.write(&ErrorLine(&format!(
                        "✗ Failed to get identities for context {}: {}. Skipping.", 
                        context_id, 
                        err
                    )));
                    error_count += 1;
                    continue;
                }
            };

            let request = UpdateContextApplicationRequest::new(new_application_id, executor_id);

            match client.update_context_application(context_id, request).await {
                Ok(response) => {
                    environment.output.write(&InfoLine(&format!(
                        "✓ Updated context {} with application {}",
                        context_id, new_application_id
                    )));
                    environment.output.write(&response);
                    success_count += 1;
                }
                Err(err) => {
                    environment.output.write(&ErrorLine(&format!(
                        "✗ Failed to update context {}: {}",
                        context_id, err
                    )));
                    error_count += 1;
                }
            }
        }

        environment.output.write(&InfoLine(&format!(
            "📊 Update complete: {} successful, {} failed",
            success_count, error_count
        )));

        // If we had successful updates, update the baseline to track the new app ID
        if success_count > 0 {
            baseline_app_id = new_application_id;
            environment.output.write(&InfoLine(&format!(
                "🔄 Updated baseline tracking to application {}", 
                baseline_app_id
            )));
        }
    }

    Ok(())
}
