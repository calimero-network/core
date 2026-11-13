//! mero-sign: CLI tool for signing Calimero bundle manifests. Kept for one release:
//! the commands are `meroctl app sign`, `meroctl app key new|id` (and `meroctl app pack`).
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

        /// The previous release (.mpk, component .wasm or ABI JSON): warn about ABI changes
        /// that break callers, and refuse a state change without a state version bump
        #[arg(long)]
        previous: Option<PathBuf>,
    },

    /// Generate a new Ed25519 keypair
    GenerateKey {
        /// Output path for the key file
        #[arg(long, short)]
        output: PathBuf,

        /// Replace an existing key file (the old key becomes unrecoverable)
        #[arg(long)]
        force: bool,
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
    eprintln!("mero-sign is deprecated: use `meroctl app sign`, `meroctl app key new` or `meroctl app key id`");

    match cli.command {
        Commands::Sign {
            manifest,
            key,
            dev,
            previous,
        } => {
            if let Some(previous) = previous {
                let check = mero_sign::check_abi(&manifest, &previous)?;
                eprintln!("ABI changes since {}:", previous.display());
                eprint!("{}", check.report);
                if check.breaking {
                    eprintln!(
                        "\u{26a0}  This release breaks existing callers of the app (see `breaking` above)."
                    );
                }
            }
            let signing_key = if dev {
                mero_sign::dev_signing_key()
            } else if let Some(key_path) = key {
                mero_sign::load_signing_key(&key_path)?
            } else {
                bail!("either --key <path> or --dev must be specified")
            };
            mero_sign::sign_manifest(&manifest, &signing_key)
        }
        Commands::GenerateKey { output, force } => mero_sign::generate_key(&output, force),
        Commands::DeriveSignerId { key } => mero_sign::derive_signer_id(&key),
    }
}
