//! mero-sign: CLI tool for signing Calimero bundle manifests
//!
//! This tool implements the signing flow:
//! - Generate Ed25519 keypairs
//! - Sign manifests using RFC 8785 canonicalization
//! - Derive did:key signerId from public keys

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use eyre::{bail, Result};

#[derive(Parser)]
#[command(name = "mero-sign")]
#[command(about = "Sign Calimero bundle manifests")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Sign a manifest.json file in-place
    Sign {
        /// Path to the manifest.json file
        manifest: PathBuf,

        /// Path to the key file (JSON format)
        #[arg(long, short, conflicts_with = "dev")]
        key: Option<PathBuf>,

        /// Sign with the well-known development key (cannot be published to registry)
        #[arg(long, conflicts_with = "key")]
        dev: bool,
    },

    /// Generate a new Ed25519 keypair
    GenerateKey {
        /// Output path for the key file
        #[arg(long, short)]
        output: PathBuf,
    },

    /// Derive the did:key signerId from a key file
    DeriveSignerId {
        /// Path to the key file
        #[arg(long, short)]
        key: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Sign { manifest, key, dev } => {
            let (signing_key, is_dev) = if dev {
                (mero_sign::dev_signing_key(), true)
            } else if let Some(key_path) = key {
                (mero_sign::load_signing_key(&key_path)?, false)
            } else {
                bail!("either --key <path> or --dev must be specified")
            };
            mero_sign::sign_manifest(&manifest, &signing_key, is_dev)
        }
        Commands::GenerateKey { output } => mero_sign::generate_key(&output),
        Commands::DeriveSignerId { key } => mero_sign::derive_signer_id(&key),
    }
}
