//! Library crate backing the `cargo-mero` binary, so integration tests under
//! `tests/` can reach `manifest` and `meta` directly instead of via a subprocess.

use std::path::PathBuf;

use camino::Utf8PathBuf;
use clap::Parser;

mod abi;
mod build;
mod bundle;
mod guide;
mod icon;
mod logo;
pub mod manifest;
pub mod meta;
mod new;
mod registry;
mod templates;
mod test_cmd;
mod workspace;

/// The calimero-sdk / calimero-wasm-abi version the toolchain scaffolds and
/// tests against; see "Bumping the SDK version" in the README before changing it.
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
    /// Publish a signed .mpk bundle to the app registry
    Publish(PublishArgs),
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

    /// Skip ABI extraction and embedding: the wasm carries no calimero_abi_v1 section
    #[arg(long)]
    no_abi: bool,

    #[command(flatten)]
    features: workspace::FeatureArgs,
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
    #[arg(long, conflicts_with = "bump")]
    app_version: Option<String>,

    /// Fetch the next appVersion from the registry: a patch/minor/major bump
    /// over the highest version published for this package
    #[arg(long, value_enum, conflicts_with = "app_version")]
    bump: Option<BumpArg>,

    /// Override the manifest package id (reverse-DNS app id, e.g. for building a
    /// migration-target bundle under a distinct identity)
    #[arg(long)]
    package: Option<String>,

    /// Output path for the .mpk bundle (defaults to dist/<package>-<appVersion>.mpk)
    #[arg(short, long)]
    output: Option<Utf8PathBuf>,

    /// Build without wasm-opt size optimization, keeping debug/profiling info
    #[arg(long)]
    profiling: bool,

    /// Path to the app's Cargo.toml
    #[arg(long)]
    manifest_path: Option<Utf8PathBuf>,

    /// Skip ABI extraction/embedding and omit the abi artifact from the manifest
    /// (the bundle cannot be migrated)
    #[arg(long)]
    no_abi: bool,

    /// Ship without an icon (fine when `frontend` is set: the desktop discovers
    /// a PWA icon at that URL)
    #[arg(long)]
    no_icon: bool,

    /// Suppress the icon preview printed above the summary
    #[arg(long)]
    no_logo: bool,

    /// Print the built .mpk path as the last line of output, for scripts that
    /// need it without reconstructing the versioned filename themselves
    #[arg(long)]
    print_output_path: bool,

    #[command(flatten)]
    features: workspace::FeatureArgs,
}

/// CLI spelling of `registry::Bump`, kept separate so `registry` stays
/// independent of clap.
#[derive(clap::ValueEnum, Clone, Copy)]
enum BumpArg {
    Major,
    Minor,
    Patch,
}

impl From<BumpArg> for registry::Bump {
    fn from(bump: BumpArg) -> Self {
        match bump {
            BumpArg::Major => registry::Bump::Major,
            BumpArg::Minor => registry::Bump::Minor,
            BumpArg::Patch => registry::Bump::Patch,
        }
    }
}

// Mirrors mero-abi/mero-sign's own clap types rather than sharing them: both are
// published crates, and clap in their public API would make its bump breaking.
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
    /// Diff two state-schema.json versions; flags breaking and unsafe identity
    /// downgrades (e.g. an AuthoredMap/AuthoredVector field replaced by a plain type)
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

#[derive(clap::Args)]
struct PublishArgs {
    /// Path to the signed .mpk bundle
    mpk: Utf8PathBuf,
}

/// Environment variable naming the registry API key, required to publish.
const API_KEY_ENV: &str = "CALIMERO_API_KEY";

pub fn run() -> eyre::Result<()> {
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
        Command::Publish(args) => dispatch_publish(&args),
        Command::Guide => unreachable!("handled in run before dispatch"),
    }
}

fn dispatch_abi(cmd: AbiCommand) -> eyre::Result<()> {
    match cmd {
        AbiCommand::Extract { wasm_file, output } => {
            mero_abi::extract_abi(&wasm_file, output.as_deref())
        }
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
    let signing_key = bundle::resolve_signing_key(args.key.as_deref(), args.dev)?;
    mero_sign::sign_manifest(&args.manifest, &signing_key)
}

fn dispatch_publish(args: &PublishArgs) -> eyre::Result<()> {
    let api_key = std::env::var(API_KEY_ENV)
        .map_err(|_| eyre::eyre!("{API_KEY_ENV} must be set to publish"))?;
    registry::publish(&registry::base_url(), &api_key, &args.mpk)?;
    println!("published {}", args.mpk);
    Ok(())
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
    fn bundle_bump_and_app_version_are_mutually_exclusive() {
        assert!(Cargo::try_parse_from(["cargo", "mero", "bundle", "--bump", "patch"]).is_ok());
        assert!(Cargo::try_parse_from([
            "cargo",
            "mero",
            "bundle",
            "--bump",
            "patch",
            "--app-version",
            "1.0.0"
        ])
        .is_err());
    }

    #[test]
    fn publish_subcommand_parses() {
        assert!(Cargo::try_parse_from(["cargo", "mero", "publish", "dist/app-1.0.0.mpk"]).is_ok());
    }

    #[test]
    fn feature_flags_parse_on_both_build_and_bundle() {
        // Cargo's own spelling: comma or space separated, and repeatable.
        for cmd in ["build", "bundle"] {
            assert!(Cargo::try_parse_from(["cargo", "mero", cmd, "--features", "a,b"]).is_ok());
            assert!(Cargo::try_parse_from(["cargo", "mero", cmd, "--features", "a b"]).is_ok());
            assert!(Cargo::try_parse_from([
                "cargo",
                "mero",
                cmd,
                "--features",
                "a",
                "--features",
                "b",
                "--no-default-features"
            ])
            .is_ok());
        }
    }

    #[test]
    fn bundle_rejects_the_removed_unsigned_flag() {
        assert!(Cargo::try_parse_from(["cargo", "mero", "bundle", "--unsigned"]).is_err());
    }

    #[test]
    fn abi_subcommands_parse() {
        assert!(Cargo::try_parse_from(["cargo", "mero", "abi", "extract", "app.wasm"]).is_ok());
        assert!(Cargo::try_parse_from([
            "cargo", "mero", "abi", "extract", "app.wasm", "-o", "out.json"
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
