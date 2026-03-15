use calimero_config::ConfigFile;
use clap::{Parser, Subcommand};
use eyre::{bail, eyre, Result as EyreResult, WrapErr};
use url::Url;

use crate::cli::RootArgs;
use crate::kms::{self, KmsProbeResult};

#[derive(Debug, Parser)]
pub struct KmsCommand {
    #[command(subcommand)]
    action: KmsSubcommands,
}

#[derive(Debug, Subcommand)]
enum KmsSubcommands {
    /// Probe KMS attestation and key-fetch flow
    Probe(KmsProbeCommand),
}

#[derive(Debug, Parser)]
pub struct KmsProbeCommand {
    /// Override configured KMS URL for this probe run
    #[arg(long, value_name = "URL")]
    kms_url: Option<Url>,
    /// Emit machine-readable probe result as JSON
    #[arg(long, default_value_t = false)]
    json: bool,
}

impl KmsCommand {
    pub async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        match self.action {
            KmsSubcommands::Probe(command) => command.run(root_args).await,
        }
    }
}

impl KmsProbeCommand {
    async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        let kms_url = self.kms_url;
        let json = self.json;
        let path = root_args.home.join(&root_args.node_name);
        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config = ConfigFile::load(&path)
            .await
            .wrap_err("Failed to load node configuration")?;

        let mut kms_config = config
            .tee
            .as_ref()
            .ok_or_else(|| eyre!("TEE is not configured in this node"))?
            .kms
            .clone();
        let phala = kms_config
            .phala
            .as_mut()
            .ok_or_else(|| eyre!("tee.kms.phala is not configured"))?;

        if let Some(kms_url) = kms_url {
            phala.url = kms_url;
        }

        phala.attestation = kms::resolve_effective_attestation_config(&phala.attestation).wrap_err(
            "Failed to resolve tee.kms.phala.attestation policy (including external policy_json_path)",
        )?;

        let peer_id = config.identity.public().to_peer_id().to_base58();
        let result = kms::probe_storage_key(&kms_config, &peer_id, &config.identity).await;
        print_result(json, &result)?;

        if result.ok {
            return Ok(());
        }

        bail!(
            "KMS probe failed at stage={:?} code={}",
            result.stage,
            result.code
        );
    }
}

fn print_result(json: bool, result: &KmsProbeResult) -> EyreResult<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(result).wrap_err("Failed to serialize probe result")?
        );
        return Ok(());
    }

    if result.ok {
        println!(
            "KMS probe succeeded: stage={:?} code={} details={}",
            result.stage,
            result.code,
            result.details.as_deref().unwrap_or_default()
        );
    } else {
        eprintln!(
            "KMS probe failed: stage={:?} code={} kms_error={} details={}",
            result.stage,
            result.code,
            result.kms_error.as_deref().unwrap_or("-"),
            result.details.as_deref().unwrap_or("-")
        );
    }

    Ok(())
}
