use std::path::Path;
use std::time::Instant;

use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    CreateContextRequest, InstallDevApplicationRequest, UpdateContextApplicationRequest,
};
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use eyre::{bail, Result};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::cli::Environment;
use crate::client::Client;

// The dev loop's own bundle: one fixed name, rebuilt in place on every reload.
const DEV_BUNDLE_NAME: &str = "dev.mpk";
const BUNDLE_CMD: &str = "cargo mero bundle --dev --no-icon";

#[derive(Debug, Parser)]
#[command(about = "Developer workflow commands")]
pub struct DevCommand {
    #[command(subcommand)]
    pub subcommand: DevSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum DevSubCommands {
    Start(StartCommand),
}

#[derive(Debug, Parser)]
#[command(about = "Start a dev session: build, install, create context, watch")]
pub struct StartCommand {
    /// Path to a signed .mpk bundle, or a project directory to build one from
    pub path: Utf8PathBuf,

    /// Watch for file changes and auto-reinstall
    #[arg(long, short = 'w')]
    pub watch: bool,

    /// Force a new context (don't reuse existing)
    #[arg(long)]
    pub new: bool,

    /// Init params for context creation
    #[arg(long, short = 'p')]
    pub params: Option<String>,

    /// Deterministic context seed
    #[arg(long, short = 's')]
    pub seed: Option<Hash>,

    /// Skip the build step (use pre-built artifact)
    #[arg(long)]
    pub no_build: bool,

    /// Group ID (hex) to attach created contexts to
    #[arg(long, required = true)]
    pub group_id: String,
}

impl DevCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            DevSubCommands::Start(start) => start.run(environment).await,
        }
    }
}

impl StartCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        // Step 1: Resolve artifact (build if needed)
        let artifact_path = resolve_artifact(&self.path, self.no_build)?;

        // Step 3: Install app + create/reuse context
        let (application_id, context_id, member_public_key) =
            self.initial_setup(environment, &artifact_path).await?;

        // Step 4: Print summary
        self.print_summary(environment, application_id, context_id, member_public_key)
            .await?;

        // Step 5: Watch loop (blocks until ctrl-c)
        if self.watch {
            let watch_target = if self.path.as_std_path().is_dir() {
                self.path.canonicalize_utf8()?
            } else {
                artifact_path.clone()
            };
            watch_and_reload(
                environment,
                context_id,
                ReloadPaths {
                    watch_target,
                    artifact_path,
                    project_path: self.path.clone(),
                },
                self.no_build,
                member_public_key,
            )
            .await?;
        }

        Ok(())
    }

    async fn initial_setup(
        &self,
        environment: &mut Environment,
        path: &Utf8PathBuf,
    ) -> Result<(ApplicationId, ContextId, PublicKey)> {
        let client = environment.client()?;

        eprintln!("Installing application from {path}...");
        let install_response = client
            .install_dev_application(InstallDevApplicationRequest::new(path.clone()))
            .await?;
        let application_id = install_response.data.application_id;
        eprintln!("  ApplicationId: {application_id}");

        let (context_id, member_public_key, reused) = if self.new {
            eprintln!("Creating new context (--new)...");
            let request = CreateContextRequest::new(
                application_id,
                self.seed,
                self.params
                    .clone()
                    .map(String::into_bytes)
                    .unwrap_or_default(),
                self.group_id.clone(),
                None,
            );
            let response = client.create_context(request).await?;
            (
                response.data.context_id,
                response.data.member_public_key,
                false,
            )
        } else {
            find_or_create_context(
                client,
                application_id,
                self.seed,
                &self.params,
                self.group_id.clone(),
            )
            .await?
        };

        let action = if reused { "updated" } else { "created" };
        eprintln!("  Context: {context_id} ({action})");

        Ok((application_id, context_id, member_public_key))
    }

    async fn print_summary(
        &self,
        environment: &Environment,
        application_id: ApplicationId,
        context_id: ContextId,
        member_public_key: PublicKey,
    ) -> Result<()> {
        let client = environment.client()?;
        let node_url = client.api_url();

        let app_response = client.get_application(&application_id).await?;
        let app = app_response.data.application;

        let package_display = app
            .as_ref()
            .map(|a| &a.package)
            .filter(|p| !p.is_empty())
            .map_or_else(|| "<unknown>".to_owned(), |p| p.clone());

        let signer_display = app
            .as_ref()
            .and_then(|a| a.signer_id.as_ref())
            .map_or_else(|| "<none>".to_owned(), |s| s.as_str().to_owned());

        eprintln!();
        eprintln!("  Dev session ready");
        eprintln!();
        eprintln!("  Application:  {package_display}");
        eprintln!("  AppId:        {application_id}");
        eprintln!("  Context:      {context_id}");
        eprintln!("  Identity:     {member_public_key}");
        eprintln!("  Signer:       {signer_display}");
        eprintln!();
        eprintln!("  Auth URL:     {node_url}auth/login?application-id={application_id}");
        eprintln!("  JSON-RPC:     {node_url}jsonrpc");
        eprintln!();

        if self.watch {
            eprintln!("  Watching for changes...");
            eprintln!();
        }

        Ok(())
    }
}

async fn find_or_create_context(
    client: &Client,
    application_id: ApplicationId,
    seed: Option<Hash>,
    params: &Option<String>,
    group_id: String,
) -> Result<(ContextId, PublicKey, bool)> {
    let contexts_response = client.list_contexts().await?;
    let existing = contexts_response
        .data
        .contexts
        .iter()
        .find(|c| c.context.application_id == application_id);

    if let Some(ctx) = existing {
        eprintln!("Found existing context, updating application...");
        let identities = client.get_context_identities(&ctx.context.id, true).await?;
        let member_pk = *identities
            .data
            .identities
            .first()
            .ok_or_else(|| eyre::eyre!("No owned identity in context {}", ctx.context.id))?;

        let update_request = UpdateContextApplicationRequest::new(application_id, member_pk);
        let _update_response = client
            .update_context_application(&ctx.context.id, update_request)
            .await?;

        Ok((ctx.context.id, member_pk, true))
    } else {
        eprintln!("No existing context found, creating new one...");
        let request = CreateContextRequest::new(
            application_id,
            seed,
            params.clone().map(String::into_bytes).unwrap_or_default(),
            group_id,
            None,
        );
        let response = client.create_context(request).await?;
        Ok((
            response.data.context_id,
            response.data.member_public_key,
            false,
        ))
    }
}

fn resolve_artifact(input: &Utf8PathBuf, no_build: bool) -> Result<Utf8PathBuf> {
    let std_path = input.as_std_path();

    if std_path.is_file() {
        if std_path.extension().and_then(|e| e.to_str()) == Some("mpk") {
            return input.canonicalize_utf8().map_err(Into::into);
        }
        bail!("Unsupported file type: {input} (expected a signed .mpk; run `{BUNDLE_CMD}`)");
    }

    if !std_path.is_dir() {
        bail!("Path does not exist: {input}");
    }

    let cargo_toml = std_path.join("Cargo.toml");
    if !cargo_toml.exists() {
        bail!("Directory {input} has no Cargo.toml — cannot build. Pass a .mpk directly.");
    }

    let output = dev_bundle_path(std_path)?;
    if no_build {
        if output.as_std_path().is_file() {
            return Ok(output);
        }
        bail!("No bundle at {output}; drop --no-build, or run `{BUNDLE_CMD} --output {output}`");
    }

    build_bundle(std_path, &output)?;
    Ok(output)
}

/// One fixed path per project, so the reload loop reinstalls what it just built
/// rather than whatever `dist/` happens to hold.
fn dev_bundle_path(project_dir: &Path) -> Result<Utf8PathBuf> {
    let canonical = Utf8PathBuf::try_from(project_dir.canonicalize()?)?;
    Ok(canonical.join("dist").join(DEV_BUNDLE_NAME))
}

fn build_bundle(project_dir: &Path, output: &Utf8PathBuf) -> Result<()> {
    eprintln!("Building bundle ({BUNDLE_CMD})...");

    let status = std::process::Command::new("cargo")
        .args(["mero", "bundle", "--dev", "--no-icon", "--output"])
        .arg(output)
        .current_dir(project_dir)
        .status()?;

    if !status.success() {
        bail!("cargo mero bundle failed with exit code {status}");
    }

    Ok(())
}

async fn watch_and_reload(
    environment: &mut Environment,
    context_id: ContextId,
    paths: ReloadPaths,
    no_build: bool,
    member_public_key: PublicKey,
) -> Result<()> {
    let ReloadPaths {
        watch_target,
        artifact_path,
        project_path,
    } = paths;
    let is_project = project_path.as_std_path().is_dir();
    let watch_dir = if is_project {
        let src = watch_target.join("src");
        if src.as_std_path().is_dir() {
            src
        } else {
            watch_target.clone()
        }
    } else {
        watch_target.clone()
    };

    let (tx, mut rx) = mpsc::channel(32);

    let mut watcher = notify::recommended_watcher(move |evt| {
        if tx.try_send(evt).is_err() {
            eprintln!("  (file watcher event dropped — build in progress)");
        }
    })?;

    let recursive = if is_project {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    watcher.watch(watch_dir.as_std_path(), recursive)?;

    eprintln!("  Watching {watch_dir} for changes...");
    eprintln!();

    while let Some(event) = rx.recv().await {
        let event = match event {
            Ok(event) => event,
            Err(err) => {
                eprintln!("  watch error: {err:?}");
                continue;
            }
        };

        match event.kind {
            EventKind::Modify(ModifyKind::Data(_)) | EventKind::Create(_) => {}
            EventKind::Remove(_) => continue,
            EventKind::Any | EventKind::Access(_) | EventKind::Modify(_) | EventKind::Other => {
                continue
            }
        }

        // Debounce: drain any additional events that arrive within 500ms
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        while rx.try_recv().is_ok() {}

        let start = Instant::now();

        let install_path = if is_project && !no_build {
            match dev_bundle_path(project_path.as_std_path()).and_then(|output| {
                build_bundle(project_path.as_std_path(), &output).map(|()| output)
            }) {
                Ok(output) => output,
                Err(err) => {
                    eprintln!("  Build failed: {err}");
                    continue;
                }
            }
        } else {
            artifact_path.clone()
        };

        let client = environment.client()?;

        let install_response = match client
            .install_dev_application(InstallDevApplicationRequest::new(install_path))
            .await
        {
            Ok(r) => r,
            Err(err) => {
                eprintln!("  Install failed: {err}");
                continue;
            }
        };
        let application_id = install_response.data.application_id;

        let request = UpdateContextApplicationRequest::new(application_id, member_public_key);
        match client
            .update_context_application(&context_id, request)
            .await
        {
            Ok(_) => {
                let elapsed = start.elapsed();
                eprintln!(
                    "  \u{21bb} Reloaded in {:.1}s \u{2014} context {context_id}",
                    elapsed.as_secs_f64()
                );
            }
            Err(err) => {
                eprintln!("  Context update failed: {err}");
            }
        }
    }

    Ok(())
}

/// The watch / build-output / project paths threaded into [`watch_and_reload`].
struct ReloadPaths {
    watch_target: Utf8PathBuf,
    artifact_path: Utf8PathBuf,
    project_path: Utf8PathBuf,
}
