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
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::cli::Environment;
use crate::client::Client;
use crate::sandbox::{self, DevSandbox};

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
#[command(about = "Start a dev session: sandbox, build, install, create context, watch")]
pub struct StartCommand {
    /// Path to .wasm, .mpk bundle, or project directory with manifest.json
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

    /// Node home directory (for config patching). Defaults to ~/.calimero/default
    #[arg(long, value_name = "PATH")]
    pub node_home: Option<Utf8PathBuf>,

    /// Application metadata (JSON string)
    #[arg(long)]
    pub metadata: Option<String>,
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
        // Step 1: Ensure sandbox is running
        let _sandbox = self.ensure_sandbox().await?;

        // Step 2: Resolve artifact (build if needed)
        let artifact_path = resolve_artifact(&self.path, self.no_build)?;

        let metadata = self
            .metadata
            .clone()
            .map(String::into_bytes)
            .unwrap_or_default();

        // Step 3: Install app + create/reuse context
        let (application_id, context_id, member_public_key) = self
            .initial_setup(environment, &artifact_path, &metadata)
            .await?;

        // Step 4: Print summary
        self.print_summary(environment, application_id, context_id, member_public_key)?;

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
                watch_target,
                artifact_path,
                self.path.clone(),
                self.no_build,
                metadata,
                member_public_key,
            )
            .await?;
        }

        Ok(())
    }

    /// Start sandbox if not already running, patch node config.
    async fn ensure_sandbox(&self) -> Result<Option<DevSandbox>> {
        if DevSandbox::is_running().await {
            eprintln!(
                "  NEAR sandbox already running on port {}",
                DevSandbox::rpc_port()
            );
            return Ok(None);
        }

        eprintln!("Starting local NEAR sandbox...");
        let sandbox = DevSandbox::start().await?;

        // Patch node config to point at local sandbox
        let node_home = self.resolve_node_home()?;
        let config_path = node_home.join("config.toml");

        if config_path.exists() {
            let (root_account, root_secret) = sandbox.root_credentials()?;
            let node_creds = sandbox
                .create_node_account("dev-node", &root_account, &root_secret)
                .await?;

            sandbox::patch_node_config(
                config_path.as_std_path(),
                &sandbox.rpc_url,
                &sandbox.contract_id,
                &node_creds.account_id,
                &node_creds.public_key,
                &node_creds.secret_key,
            )?;
            eprintln!("  Patched {config_path}");
            eprintln!("  NOTE: restart merod for config changes to take effect");
        } else {
            eprintln!(
                "  Warning: {config_path} not found — configure your node to use rpc_url={} contract_id={}",
                sandbox.rpc_url,
                sandbox.contract_id
            );
        }

        Ok(Some(sandbox))
    }

    fn resolve_node_home(&self) -> Result<Utf8PathBuf> {
        if let Some(ref home) = self.node_home {
            return Ok(home.clone());
        }
        let default = dirs::home_dir()
            .ok_or_else(|| eyre::eyre!("Cannot determine home directory"))?
            .join(".calimero")
            .join("default");
        Utf8PathBuf::try_from(default).map_err(Into::into)
    }

    async fn initial_setup(
        &self,
        environment: &mut Environment,
        path: &Utf8PathBuf,
        metadata: &[u8],
    ) -> Result<(ApplicationId, ContextId, PublicKey)> {
        let client = environment.client()?;

        eprintln!("Installing application from {path}...");
        let install_response = client
            .install_dev_application(InstallDevApplicationRequest::new(
                path.clone(),
                metadata.to_vec(),
                None,
                None,
            ))
            .await?;
        let application_id = install_response.data.application_id;
        eprintln!("  ApplicationId: {application_id}");

        let (context_id, member_public_key, reused) = if self.new {
            eprintln!("Creating new context (--new)...");
            let request = CreateContextRequest::new(
                "near".to_owned(),
                application_id,
                self.seed,
                self.params
                    .clone()
                    .map(String::into_bytes)
                    .unwrap_or_default(),
                None,
                None,
            );
            let response = client.create_context(request).await?;
            (
                response.data.context_id,
                response.data.member_public_key,
                false,
            )
        } else {
            find_or_create_context(client, application_id, self.seed, &self.params).await?
        };

        let action = if reused { "updated" } else { "created" };
        eprintln!("  Context: {context_id} ({action})");

        Ok((application_id, context_id, member_public_key))
    }

    fn print_summary(
        &self,
        environment: &Environment,
        application_id: ApplicationId,
        context_id: ContextId,
        member_public_key: PublicKey,
    ) -> Result<()> {
        let client = environment.client()?;
        let node_url = client.api_url();

        let app_response = tokio::task::block_in_place(|| {
            Handle::current().block_on(client.get_application(&application_id))
        })?;
        let app = app_response.data.application;

        let package_display = app
            .as_ref()
            .map(|a| &a.package)
            .filter(|p| !p.is_empty())
            .map_or_else(|| "<unknown>".to_owned(), |p| p.clone());

        let signer_display = app
            .as_ref()
            .map(|a| &a.signer_id)
            .filter(|s| !s.is_empty())
            .map_or_else(|| "<none>".to_owned(), |s| s.clone());

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
) -> Result<(ContextId, PublicKey, bool)> {
    let contexts_response = client.list_contexts().await?;
    let existing = contexts_response
        .data
        .contexts
        .iter()
        .find(|c| c.application_id == application_id);

    if let Some(ctx) = existing {
        eprintln!("Found existing context, updating application...");
        let identities = client.get_context_identities(&ctx.id, true).await?;
        let member_pk = *identities
            .data
            .identities
            .first()
            .ok_or_else(|| eyre::eyre!("No owned identity in context {}", ctx.id))?;

        let update_request = UpdateContextApplicationRequest::new(application_id, member_pk);
        let _update_response = client
            .update_context_application(&ctx.id, update_request)
            .await?;

        Ok((ctx.id, member_pk, true))
    } else {
        eprintln!("No existing context found, creating new one...");
        let request = CreateContextRequest::new(
            "near".to_owned(),
            application_id,
            seed,
            params.clone().map(String::into_bytes).unwrap_or_default(),
            None,
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
        let ext = std_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext == "wasm" || ext == "mpk" {
            return input.canonicalize_utf8().map_err(Into::into);
        }
        bail!("Unsupported file type: {input} (expected .wasm or .mpk)");
    }

    if !std_path.is_dir() {
        bail!("Path does not exist: {input}");
    }

    let cargo_toml = std_path.join("Cargo.toml");
    if !cargo_toml.exists() {
        bail!("Directory {input} has no Cargo.toml — cannot build. Pass a .wasm or .mpk directly.");
    }

    if no_build {
        return find_wasm_in_project(std_path);
    }

    build_rust_wasm(std_path)?;
    find_wasm_in_project(std_path)
}

fn build_rust_wasm(project_dir: &Path) -> Result<()> {
    eprintln!("Building WASM (cargo build --target wasm32-unknown-unknown --release)...");

    let status = std::process::Command::new("cargo")
        .args(["build", "--target", "wasm32-unknown-unknown", "--release"])
        .current_dir(project_dir)
        .status()?;

    if !status.success() {
        bail!("cargo build failed with exit code {status}");
    }

    Ok(())
}

fn find_wasm_in_project(project_dir: &Path) -> Result<Utf8PathBuf> {
    let res_dir = project_dir.join("res");
    if res_dir.is_dir() {
        if let Some(wasm) = find_first_wasm_in(&res_dir)? {
            return Ok(wasm);
        }
    }

    let target_dir = project_dir.join("target/wasm32-unknown-unknown/release");
    if target_dir.is_dir() {
        if let Some(wasm) = find_first_wasm_in(&target_dir)? {
            return Ok(wasm);
        }
    }

    bail!(
        "No .wasm file found in {}/res/ or {}/target/wasm32-unknown-unknown/release/",
        project_dir.display(),
        project_dir.display()
    )
}

fn find_first_wasm_in(dir: &Path) -> Result<Option<Utf8PathBuf>> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("wasm") {
            let utf8 = Utf8PathBuf::try_from(path)?;
            return Ok(Some(utf8));
        }
    }
    Ok(None)
}

async fn watch_and_reload(
    environment: &mut Environment,
    context_id: ContextId,
    watch_target: Utf8PathBuf,
    artifact_path: Utf8PathBuf,
    project_path: Utf8PathBuf,
    no_build: bool,
    metadata: Vec<u8>,
    member_public_key: PublicKey,
) -> Result<()> {
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

    let (tx, mut rx) = mpsc::channel(1);

    let handle = Handle::current();
    let mut watcher = notify::recommended_watcher(move |evt| {
        handle.block_on(async {
            drop(tx.send(evt).await);
        });
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

        let start = Instant::now();

        let install_path = if is_project && !no_build {
            match build_rust_wasm(project_path.as_std_path()) {
                Ok(()) => match find_wasm_in_project(project_path.as_std_path()) {
                    Ok(p) => p,
                    Err(err) => {
                        eprintln!("  Build succeeded but WASM not found: {err}");
                        continue;
                    }
                },
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
            .install_dev_application(InstallDevApplicationRequest::new(
                install_path,
                metadata.clone(),
                None,
                None,
            ))
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
