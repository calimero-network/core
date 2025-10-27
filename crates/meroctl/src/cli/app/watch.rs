use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{
    InstallDevApplicationRequest, UpdateContextApplicationRequest,
};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::Result;
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

    /// Package name (e.g., com.example.myapp)
    #[arg(
        long,
        help = "Package name (e.g., com.example.myapp)",
        default_value = "unknown"
    )]
    pub package: String,

    /// Version (e.g., 1.0.0)
    #[arg(long, help = "Version (e.g., 1.0.0)", default_value = "0.0.0")]
    pub version: String,
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
                Some(self.package.clone()),
                Some(self.version.clone()),
            ))
            .await?
            .data
            .application_id;

        // Determine which application ID to track
        let target_app_id = self.current_app_id.unwrap_or(initial_app_id);

        // Get all contexts that use this application
        let target_contexts = get_contexts_using_application(&client, &target_app_id).await?;

        if target_contexts.is_empty() {
            return handle_no_contexts_found(environment, &target_app_id);
        }

        environment.output.write(&InfoLine(&format!(
            "Found {} context(s) using application {}: {:?}",
            target_contexts.len(),
            target_app_id,
            target_contexts
        )));

        // Start watching the file
        watch_app_and_update_contexts(
            environment,
            target_app_id,
            self.path,
            metadata,
            self.package,
            self.version,
        )
        .await
    }
}

async fn watch_app_and_update_contexts(
    environment: &mut Environment,
    mut baseline_app_id: ApplicationId,
    path: Utf8PathBuf,
    metadata: Option<Vec<u8>>,
    package: String,
    version: String,
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
                Some(package.clone()),
                Some(version.clone()),
            ))
            .await?
            .data
            .application_id;

        environment.output.write(&InfoLine(&format!(
            "📦 Installed new application version: {}",
            new_application_id
        )));

        // Refresh context list to catch any new contexts using the baseline app
        let target_contexts = get_contexts_using_application(&client, &baseline_app_id).await?;

        if target_contexts.is_empty() {
            environment.output.write(&InfoLine(&format!(
                "No contexts currently use application {}.",
                baseline_app_id
            )));
            return handle_no_contexts_found(environment, &baseline_app_id);
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
                        context_id, err
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
        } else {
            // If all updates failed, the contexts are still using the old app ID
            // Continue tracking the old baseline for next iteration
            environment.output.write(&InfoLine(&format!(
                "All context updates failed. Continuing to track application {}",
                baseline_app_id
            )));
        }
    }

    Ok(())
}

/// Helper function to get all contexts that use a specific application
async fn get_contexts_using_application(
    client: &crate::client::Client,
    app_id: &ApplicationId,
) -> Result<Vec<calimero_primitives::context::ContextId>> {
    let contexts_response = client.list_contexts().await?;
    let target_contexts: Vec<_> = contexts_response
        .data
        .contexts
        .into_iter()
        .filter(|context| context.application_id == *app_id)
        .map(|context| context.id)
        .collect();
    Ok(target_contexts)
}

/// Helper function to provide consistent messaging when no contexts are found
fn handle_no_contexts_found(environment: &mut Environment, app_id: &ApplicationId) -> Result<()> {
    environment.output.write(&ErrorLine(&format!(
        "No contexts found using application {}.",
        app_id
    )));
    environment.output.write(&InfoLine(
        "To use this watch command, first create contexts with this application:",
    ));
    environment.output.write(&InfoLine(&format!(
        "  meroctl context create --application-id {} --protocol <protocol_name>",
        app_id
    )));
    environment
        .output
        .write(&InfoLine("Then run the watch command again."));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{Format, Output};
    use calimero_primitives::application::ApplicationId;

    #[test]
    fn test_handle_no_contexts_found_returns_ok() {
        let output = Output::new(Format::Human);
        let mut environment = Environment::new(output, None).unwrap();
        let app_id = ApplicationId::from([1u8; 32]);

        let result = handle_no_contexts_found(&mut environment, &app_id);
        assert!(result.is_ok());
    }

    #[test]
    fn test_watch_command_parsing_minimal() {
        use clap::Parser;

        let cmd = WatchCommand::try_parse_from(&["watch", "--path", "./test.wasm"]).unwrap();

        assert_eq!(cmd.path.as_str(), "./test.wasm");
        assert_eq!(cmd.metadata, None);
        assert_eq!(cmd.current_app_id, None);
    }

    #[test]
    fn test_watch_command_parsing_with_metadata() {
        use clap::Parser;

        let cmd = WatchCommand::try_parse_from(&[
            "watch",
            "--path",
            "./test.wasm",
            "--metadata",
            r#"{"version": "1.0.0"}"#,
        ])
        .unwrap();

        assert_eq!(cmd.path.as_str(), "./test.wasm");
        assert_eq!(cmd.metadata, Some(r#"{"version": "1.0.0"}"#.to_string()));
        assert_eq!(cmd.current_app_id, None);
    }

    #[test]
    fn test_watch_command_parsing_with_current_app_id() {
        use clap::Parser;

        let app_id = ApplicationId::from([42u8; 32]);
        let cmd = WatchCommand::try_parse_from(&[
            "watch",
            "--path",
            "./test.wasm",
            "--current-app-id",
            &app_id.to_string(),
        ])
        .unwrap();

        assert_eq!(cmd.path.as_str(), "./test.wasm");
        assert_eq!(cmd.metadata, None);
        assert_eq!(cmd.current_app_id, Some(app_id));
    }

    #[test]
    fn test_watch_command_parsing_all_options() {
        use clap::Parser;

        let app_id = ApplicationId::from([1u8; 32]);
        let cmd = WatchCommand::try_parse_from(&[
            "watch",
            "--path",
            "./test.wasm",
            "--metadata",
            "{}",
            "--current-app-id",
            &app_id.to_string(),
        ])
        .unwrap();

        assert_eq!(cmd.path.as_str(), "./test.wasm");
        assert_eq!(cmd.metadata, Some("{}".to_string()));
        assert_eq!(cmd.current_app_id, Some(app_id));
    }

    #[test]
    fn test_watch_command_parsing_short_flags() {
        use clap::Parser;

        let cmd = WatchCommand::try_parse_from(&["watch", "-p", "./test.wasm"]).unwrap();

        assert_eq!(cmd.path.as_str(), "./test.wasm");
    }

    #[test]
    fn test_watch_command_missing_path_fails() {
        use clap::Parser;

        let result = WatchCommand::try_parse_from(&["watch"]);
        assert!(result.is_err(), "Command should fail when path is missing");
    }

    #[test]
    fn test_watch_command_invalid_app_id_fails() {
        use clap::Parser;

        let result = WatchCommand::try_parse_from(&[
            "watch",
            "--path",
            "./test.wasm",
            "--current-app-id",
            "invalid-app-id",
        ]);
        assert!(
            result.is_err(),
            "Command should fail with invalid application ID"
        );
    }

    #[test]
    fn test_application_id_from_bytes_consistency() {
        // Test that we can consistently create and compare ApplicationIds
        let bytes = [42u8; 32];
        let app_id1 = ApplicationId::from(bytes);
        let app_id2 = ApplicationId::from(bytes);

        assert_eq!(app_id1, app_id2);
        assert_eq!(app_id1.to_string(), app_id2.to_string());
    }

    // Note: get_contexts_using_application function would require mocking the client
    // for proper unit testing, which is beyond the scope of current simple unit tests.
    // This function is tested implicitly through the main functionality.
}
