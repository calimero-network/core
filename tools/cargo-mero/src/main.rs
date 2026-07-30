use std::path::PathBuf;

use camino::Utf8PathBuf;
use clap::Parser;
use eyre::bail;

mod build;
mod bundle;
mod guide;
mod manifest;
mod meta;
mod new;
mod templates;
mod test_cmd;
mod workspace;

/// The calimero-sdk / calimero-wasm-abi version the toolchain scaffolds and
/// tests against. Bumping the SDK touches several pinned copies of this string;
/// see the "Bumping the SDK version" note in the README for the full list.
pub const DEFAULT_SDK_VERSION: &str = "0.11.0-rc.17";

#[derive(Parser)]
#[command(name = "cargo", bin_name = "cargo")]
enum Cargo {
    #[command(subcommand_required = false, arg_required_else_help = false)]
    Mero(MeroCli),
}

#[derive(clap::Args)]
#[command(version, about = "Calimero application toolchain", long_about = guide::ABOUT_LONG)]
struct MeroCli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Scaffold a new Calimero app (Cargo.toml, build.rs, lib.rs with a TestHost test)
    New(NewArgs),
    /// Compile to wasm32, optimize, and embed the ABI into res/<name>.wasm (the calimero_abi_v1 section)
    Build(BuildArgs),
    /// Run the node-free native tests (TestHost unit tests + convergence tests)
    Test(TestArgs),
    /// Build all services and package a signed .mpk bundle
    Bundle(BundleArgs),
    /// ABI utilities (extract, state, types, inspect, embed, diff)
    #[command(subcommand)]
    Abi(AbiCommand),
    /// Signing-key utilities (generate, derive-signer-id)
    #[command(subcommand)]
    Key(KeyCommand),
    /// Sign a bundle manifest.json in place
    Sign(SignArgs),
    /// Print the end-to-end workflow guide
    Guide,
}

#[derive(clap::Args)]
struct NewArgs {
    /// Name of the new app (crate name and default scaffold directory)
    name: String,

    /// calimero-wasm-abi / SDK version to scaffold against
    #[arg(long, default_value = DEFAULT_SDK_VERSION)]
    sdk_version: String,

    /// Directory to scaffold into (defaults to `./<name>`)
    #[arg(long)]
    path: Option<Utf8PathBuf>,
}

#[derive(clap::Args)]
struct BuildArgs {
    /// Build without wasm-opt size optimization, keeping debug/profiling info
    #[arg(long)]
    profiling: bool,

    /// Package to build, for workspaces with multiple apps
    #[arg(short, long)]
    package: Option<String>,

    /// Path to the app's Cargo.toml
    #[arg(long)]
    manifest_path: Option<Utf8PathBuf>,
}

#[derive(clap::Args)]
struct TestArgs {
    /// Path to the app's Cargo.toml
    #[arg(long)]
    manifest_path: Option<Utf8PathBuf>,

    /// Extra arguments forwarded to the test binary after `--` (e.g. a name
    /// filter or `--nocapture`), not to `cargo test` itself
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(clap::Args)]
struct BundleArgs {
    /// Sign with a production Ed25519 key file (see `cargo mero key generate`)
    #[arg(long, group = "signing")]
    key: Option<PathBuf>,

    /// Sign with the well-known development key: fine locally, REFUSED by the registry
    #[arg(long, group = "signing")]
    dev: bool,

    /// Override the app version recorded in manifest.json
    #[arg(long)]
    app_version: Option<String>,

    /// Override the manifest package id (reverse-DNS app id, e.g. for building a
    /// migration-target bundle under a distinct identity)
    #[arg(long)]
    package: Option<String>,

    /// Output path for the .mpk bundle (defaults to dist/<package>.mpk)
    #[arg(short, long)]
    output: Option<Utf8PathBuf>,

    /// Build without wasm-opt size optimization, keeping debug/profiling info
    #[arg(long)]
    profiling: bool,

    /// Path to the app's Cargo.toml
    #[arg(long)]
    manifest_path: Option<Utf8PathBuf>,
}

// These mirror the standalone mero-abi / mero-sign binaries rather than sharing
// clap types with them: both are published crates, so clap in their public API
// would make a clap major bump a breaking release. The arms below are one lib
// call each, so drift shows up as a missing subcommand, not wrong behavior.
#[derive(clap::Subcommand)]
enum AbiCommand {
    /// Extract ABI from a WASM file
    Extract {
        /// Input WASM file
        #[arg(value_name = "WASM_FILE")]
        wasm_file: PathBuf,

        /// Output JSON file
        #[arg(short, long, value_name = "OUTPUT")]
        output: Option<PathBuf>,

        /// Verify ABI using get_abi* exports
        #[arg(long)]
        verify: bool,
    },
    /// Extract only the types schema from a WASM file
    Types {
        /// Input WASM file
        #[arg(value_name = "WASM_FILE")]
        wasm_file: PathBuf,

        /// Output JSON file
        #[arg(short, long, value_name = "OUTPUT")]
        output: Option<PathBuf>,
    },
    /// Extract the state schema (state root and all its type dependencies)
    State {
        /// Input WASM file
        #[arg(value_name = "WASM_FILE")]
        wasm_file: PathBuf,

        /// Output JSON file
        #[arg(short, long, value_name = "OUTPUT")]
        output: Option<PathBuf>,
    },
    /// Inspect WASM file sections
    Inspect {
        /// Input WASM file
        #[arg(value_name = "WASM_FILE")]
        wasm_file: PathBuf,
    },
    /// Embed an ABI manifest (abi.json or state-schema.json) into a wasm as the calimero_abi_v1 section (in place)
    Embed {
        /// The wasm file to modify in place
        wasm: PathBuf,
        /// The ABI manifest to embed (must be name-sorted; `cargo mero build` embeds the canonicalized full ABI)
        schema: PathBuf,
    },
    /// Diff two state-schema.json versions; flags breaking + unsafe identity
    /// downgrades (an AuthoredMap/AuthoredVector/SharedStorage field replaced by
    /// a plain type, which silently strips authorship / writer-ACL)
    Diff {
        /// The new (current build) state-schema.json
        #[arg(value_name = "CURRENT")]
        current: PathBuf,

        /// The previous (baseline) state-schema.json to compare against
        #[arg(value_name = "BASELINE")]
        baseline: PathBuf,

        /// Report findings but always exit 0 (don't fail CI)
        #[arg(long)]
        exit_zero: bool,
    },
}

#[derive(clap::Subcommand)]
enum KeyCommand {
    /// Generate a new Ed25519 keypair
    Generate {
        /// Output path for the key file
        #[arg(short, long)]
        output: PathBuf,

        /// Replace an existing key file (the old key becomes unrecoverable)
        #[arg(long)]
        force: bool,
    },
    /// Derive the did:key signerId from a key file
    DeriveSignerId {
        /// Path to the key file
        #[arg(short, long)]
        key: PathBuf,
    },
}

#[derive(clap::Args)]
struct SignArgs {
    /// Path to the manifest.json file
    manifest: PathBuf,

    /// Path to a production key file (JSON format)
    #[arg(long, short, conflicts_with = "dev")]
    key: Option<PathBuf>,

    /// Sign with the well-known development key (cannot be published to registry)
    #[arg(long, conflicts_with = "key")]
    dev: bool,
}

fn main() -> eyre::Result<()> {
    let Cargo::Mero(cli) = Cargo::parse();
    match cli.command {
        None | Some(Command::Guide) => {
            guide::print();
            Ok(())
        }
        Some(cmd) => dispatch(cmd),
    }
}

fn dispatch(cmd: Command) -> eyre::Result<()> {
    match cmd {
        Command::New(args) => new::run(&args),
        Command::Build(args) => build::run(&args).map(|_| ()),
        Command::Test(args) => test_cmd::run(&args),
        Command::Bundle(args) => bundle::run(&args).map(|_| ()),
        Command::Abi(cmd) => dispatch_abi(cmd),
        Command::Key(cmd) => dispatch_key(cmd),
        Command::Sign(args) => dispatch_sign(&args),
        Command::Guide => unreachable!("handled in main before dispatch"),
    }
}

fn dispatch_abi(cmd: AbiCommand) -> eyre::Result<()> {
    match cmd {
        AbiCommand::Extract {
            wasm_file,
            output,
            verify,
        } => mero_abi::extract_abi(&wasm_file, output.as_deref(), verify),
        AbiCommand::Types { wasm_file, output } => {
            mero_abi::extract_types_schema(&wasm_file, output.as_deref())
        }
        AbiCommand::State { wasm_file, output } => {
            mero_abi::extract_state_schema(&wasm_file, output.as_deref())
        }
        AbiCommand::Inspect { wasm_file } => mero_abi::inspect_wasm(&wasm_file),
        AbiCommand::Embed { wasm, schema } => mero_abi::run_embed(&wasm, &schema),
        AbiCommand::Diff {
            current,
            baseline,
            exit_zero,
        } => {
            if mero_abi::run_diff(&current, &baseline, exit_zero)? {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

fn dispatch_key(cmd: KeyCommand) -> eyre::Result<()> {
    match cmd {
        KeyCommand::Generate { output, force } => mero_sign::generate_key(&output, force),
        KeyCommand::DeriveSignerId { key } => mero_sign::derive_signer_id(&key),
    }
}

fn dispatch_sign(args: &SignArgs) -> eyre::Result<()> {
    let signing_key = if args.dev {
        mero_sign::dev_signing_key()
    } else if let Some(key_path) = &args.key {
        mero_sign::load_signing_key(key_path)?
    } else {
        bail!("either --key <path> or --dev must be specified")
    };
    mero_sign::sign_manifest(&args.manifest, &signing_key)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn parses_as_cargo_subcommand() {
        // cargo passes ["cargo-mero", "mero", "build"] when invoked as `cargo mero build`
        assert!(Cargo::try_parse_from(["cargo", "mero", "build"]).is_ok());
        assert!(Cargo::try_parse_from(["cargo", "mero"]).is_ok()); // bare -> guide
        assert!(Cargo::try_parse_from(["cargo", "mero", "guide"]).is_ok());
    }

    #[test]
    fn bundle_signing_flags_are_exclusive_but_orthogonal_to_other_args() {
        // The signing flags are mutually exclusive.
        assert!(
            Cargo::try_parse_from(["cargo", "mero", "bundle", "--dev", "--key", "k.json"]).is_err()
        );

        // A signing flag must still combine freely with unrelated args; guards the
        // struct-level `#[group]` regression that pulled every field into the group.
        assert!(Cargo::try_parse_from([
            "cargo",
            "mero",
            "bundle",
            "--dev",
            "--manifest-path",
            "Cargo.toml"
        ])
        .is_ok());
    }

    #[test]
    fn bundle_rejects_the_removed_unsigned_flag() {
        assert!(Cargo::try_parse_from(["cargo", "mero", "bundle", "--unsigned"]).is_err());
    }

    #[test]
    fn abi_subcommands_parse() {
        assert!(Cargo::try_parse_from(["cargo", "mero", "abi", "extract", "app.wasm"]).is_ok());
        assert!(Cargo::try_parse_from([
            "cargo", "mero", "abi", "extract", "app.wasm", "-o", "out.json", "--verify"
        ])
        .is_ok());
        assert!(Cargo::try_parse_from(["cargo", "mero", "abi", "types", "app.wasm"]).is_ok());
        assert!(Cargo::try_parse_from(["cargo", "mero", "abi", "state", "app.wasm"]).is_ok());
        assert!(Cargo::try_parse_from(["cargo", "mero", "abi", "inspect", "app.wasm"]).is_ok());
        assert!(Cargo::try_parse_from([
            "cargo",
            "mero",
            "abi",
            "embed",
            "app.wasm",
            "schema.json"
        ])
        .is_ok());
        assert!(Cargo::try_parse_from([
            "cargo",
            "mero",
            "abi",
            "diff",
            "current.json",
            "baseline.json",
            "--exit-zero"
        ])
        .is_ok());
    }

    #[test]
    fn key_subcommands_parse() {
        assert!(
            Cargo::try_parse_from(["cargo", "mero", "key", "generate", "-o", "key.json"]).is_ok()
        );
        assert!(Cargo::try_parse_from([
            "cargo",
            "mero",
            "key",
            "derive-signer-id",
            "-k",
            "key.json"
        ])
        .is_ok());
    }

    #[test]
    fn sign_args_key_and_dev_are_mutually_exclusive() {
        assert!(Cargo::try_parse_from(["cargo", "mero", "sign", "manifest.json", "--dev"]).is_ok());
        assert!(Cargo::try_parse_from([
            "cargo",
            "mero",
            "sign",
            "manifest.json",
            "--key",
            "key.json"
        ])
        .is_ok());
        assert!(Cargo::try_parse_from([
            "cargo",
            "mero",
            "sign",
            "manifest.json",
            "--dev",
            "--key",
            "key.json"
        ])
        .is_err());
    }
}
