//! `merod`: the Calimero node. v1's command line (`--home`, `--node`, `init`,
//! `run`, `config`, `kms`, and `attest`) over the node (`calimero-node`):
//! attested, one Raft log per app.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use calimero_client::Measurements;
use calimero_node::attest::Mode;
use calimero_node::system::{Authority, Governance};
use calimero_node::{kms, run, Config};
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};

mod node_config;

use node_config::NodeArgs;

#[derive(Debug, Parser)]
#[command(
    name = "merod",
    version,
    about = "Calimero node: attested, one Raft log per app"
)]
struct Cli {
    /// Directory for nodes' configuration and data.
    #[arg(long, env = "CALIMERO_HOME", default_value_os_t = default_home())]
    home: PathBuf,
    /// The node: its configuration is `{home}/{node}/config.toml`, its data
    /// `{home}/{node}/data` unless configured otherwise.
    #[arg(long, env = "MEROD_NODE_NAME")]
    node: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Write the node's configuration. Every `run` flag can be set here.
    Init {
        #[command(flatten)]
        node: NodeArgs,
        /// Replace an existing configuration.
        #[arg(long)]
        force: bool,
    },
    /// Start the node from its configuration; flags given here override it
    /// for this run (so a container can run with flags and no `init`).
    Run {
        #[command(flatten)]
        node: NodeArgs,
    },
    /// Change the node's configuration (the flags given), then print it.
    Config {
        #[command(flatten)]
        node: NodeArgs,
    },
    /// A development cluster in this process.
    Local(LocalArgs),
    /// What a stopped node's data directory holds (v1's `merodb`).
    Inspect(InspectArgs),
    /// The data-disk key from mero-kms (for a TEE boot script).
    #[command(subcommand)]
    Kms(KmsCmd),
    /// A release attestation: a quote over a nonce and this merod's release,
    /// made without any KMS key, so a manager can check the image before
    /// mero-kms releases a key to it.
    Attest(AttestArgs),
}

/// `merod attest`: one quote, or (`--serve`) one per request. Reads the
/// node's configuration if there is one, and nothing else of its state.
#[derive(Debug, Args)]
struct AttestArgs {
    /// The verifier's nonce, 32 bytes hex [default: a random one].
    #[arg(long, conflicts_with = "serve")]
    nonce: Option<String>,
    /// Write the attestation to this file instead of stdout.
    #[arg(long, conflicts_with = "serve")]
    out: Option<PathBuf>,
    /// The output format.
    #[arg(long, value_enum, default_value_t = AttestFormat::Json)]
    format: AttestFormat,
    /// Bind this libp2p identity's peer id (a protobuf keypair file; read,
    /// never created) [default: the configured `--kms-identity`, if it exists].
    #[arg(long)]
    identity: Option<PathBuf>,
    /// Serve `GET /attestation/release?nonce=` on this address until stopped
    /// (SIGINT or SIGTERM), each answer a fresh quote over the caller's nonce.
    #[arg(long)]
    serve: Option<std::net::SocketAddr>,
    /// A mock quote (development; what `run --dev` attests with).
    #[arg(long, env = "MEROD_DEV", num_args = 0..=1, default_missing_value = "true")]
    dev: Option<bool>,
    /// The mero-tee release this image is.
    #[arg(long, env = "MERO_TEE_VERSION")]
    tee_release: Option<String>,
    /// The mero-tee image profile.
    #[arg(long, env = "MERO_TEE_PROFILE")]
    tee_profile: Option<String>,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum AttestFormat {
    Json,
}

/// What this merod claims in a release attestation.
fn release_claim(
    args: &AttestArgs,
    config: &NodeArgs,
) -> eyre::Result<calimero_node::attest::release::ReleaseClaim> {
    let exe = std::env::current_exe()?;
    let binary = std::fs::read(&exe).map_err(|e| eyre::eyre!("reading {}: {e}", exe.display()))?;
    let identity = match (&args.identity, &config.kms_identity) {
        (Some(path), _) => Some(calimero_kms_client::disk_key::identity_peer_id(path)?),
        (None, Some(path)) if path.exists() => {
            Some(calimero_kms_client::disk_key::identity_peer_id(path)?)
        }
        (None, _) => None,
    };
    Ok(calimero_node::attest::release::ReleaseClaim {
        merod_version: env!("CARGO_PKG_VERSION").to_owned(),
        merod_sha256: Some(hex::encode(calimero_identity::sha256(&binary))),
        tee_release: args
            .tee_release
            .clone()
            .or_else(|| config.tee_release.clone()),
        tee_profile: args
            .tee_profile
            .clone()
            .or_else(|| config.tee_profile.clone()),
        identity,
    })
}

async fn attest(home: Option<&PathBuf>, args: AttestArgs) -> eyre::Result<()> {
    let config = effective(home, NodeArgs::default())?;
    let mode = if args.dev.or(config.dev).unwrap_or(false) {
        Mode::Mock
    } else {
        Mode::Tdx
    };
    let claim = release_claim(&args, &config)?;
    if let Some(listen) = args.serve {
        let stop = async {
            let mut term =
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(term) => term,
                    Err(_) => {
                        let _ = tokio::signal::ctrl_c().await;
                        return;
                    }
                };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        };
        eprintln!("serving GET /attestation/release on {listen}");
        return calimero_node::attest::serve_release(listen, mode, claim, stop).await;
    }
    let nonce: [u8; 32] = match &args.nonce {
        Some(h) => hex::decode(h)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| eyre::eyre!("--nonce must be 32 bytes hex"))?,
        None => calimero_identity::random::<32>(),
    };
    let doc = calimero_node::attest::release_attestation(mode, &nonce, claim)?;
    let text = match args.format {
        AttestFormat::Json => serde_json::to_string_pretty(&doc)?,
    };
    match &args.out {
        Some(path) => std::fs::write(path, text + "\n")?,
        None => println!("{text}"),
    }
    Ok(())
}

#[derive(Debug, Subcommand)]
enum KmsCmd {
    DiskKey(calimero_kms_client::disk_key::KmsDiskKeyCommand),
}

fn default_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".calimero")
}

/// `merod local`: a development cluster in this process.
#[derive(Debug, Args)]
struct LocalArgs {
    /// How many nodes.
    #[arg(long, default_value_t = 3)]
    nodes: u64,
    /// Node i listens on base-port + i.
    #[arg(long, default_value_t = 2500)]
    base_port: u16,
    /// Where the cluster keeps its state (kept across restarts).
    #[arg(long, default_value = ".merod-local")]
    dir: PathBuf,
    /// Start from empty state.
    #[arg(long)]
    fresh: bool,
    /// The Ops API token (default: a new random one, printed).
    #[arg(long, env = "MEROD_OPS_TOKEN")]
    ops_token: Option<String>,
    /// The package registry the cluster installs from.
    #[arg(long, env = "MEROD_REGISTRY_URL")]
    registry_url: Option<String>,
    /// Origins allowed to run passkey ceremonies.
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "http://localhost:3000,http://localhost:5173"
    )]
    origins: Vec<String>,
}

/// `merod inspect`: read offline. Without the storage key it reads the raw
/// layout only (sizes are sealed sizes); with it, each group's metadata
/// (vote, applied index, membership) too. Stop the node first: the store is
/// opened for writing.
#[derive(Debug, Args)]
struct InspectArgs {
    /// The data directory (default: the `--node`'s).
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// The storage key K_r, hex, in a file (it unwraps the cluster key).
    #[arg(long)]
    storage_key_file: Option<PathBuf>,
}

fn inspect(data_dir: &Path, args: &InspectArgs) -> eyre::Result<()> {
    let key: Option<[u8; 32]> = match &args.storage_key_file {
        Some(path) => Some(
            hex::decode(std::fs::read_to_string(path)?.trim())?
                .try_into()
                .map_err(|_| eyre::eyre!("the storage key must be 32 bytes"))?,
        ),
        None => None,
    };
    let out = calimero_node::inspect::inspect(data_dir, key)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

async fn local(args: LocalArgs) -> eyre::Result<()> {
    if args.fresh && args.dir.exists() {
        std::fs::remove_dir_all(&args.dir)?;
    }
    let token = args
        .ops_token
        .unwrap_or_else(|| hex::encode(calimero_identity::random::<16>()));
    let cluster = calimero_node::local::start(
        args.nodes,
        args.base_port,
        &args.dir,
        &token,
        args.origins,
        args.registry_url,
    )
    .await?;
    println!(
        "Local cluster \"{}\" (development: mock attestation, dev storage key)",
        calimero_node::local::CLUSTER
    );
    for (i, url) in cluster.urls.iter().enumerate() {
        println!("  node {}  {url}", i + 1);
    }
    println!("  state     {}", args.dir.display());
    println!("  ops token {token}");
    println!();
    println!("  meroctl node add local {} --allow-mock", cluster.urls[0]);
    println!("  meroctl --node local login --dev-passkey ./dev-passkey.json");
    tokio::signal::ctrl_c().await?;
    cluster.stop().await;
    Ok(())
}

/// The `[outbound]` table of the configuration (or of `--settings`).
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    outbound: calimero_outbound::OutboundSettings,
}

fn read_settings(path: &Path) -> eyre::Result<calimero_outbound::OutboundSettings> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| eyre::eyre!("reading {}: {e}", path.display()))?;
    let settings: Settings =
        toml::from_str(&text).map_err(|e| eyre::eyre!("{}: {e}", path.display()))?;
    Ok(settings.outbound)
}

fn need_node(home: Option<&PathBuf>, what: &str) -> eyre::Result<PathBuf> {
    home.cloned()
        .ok_or_else(|| eyre::eyre!("{what} needs the node: --node NAME"))
}

/// The configuration `run` starts from: the node's file, then the flags.
fn effective(home: Option<&PathBuf>, given: NodeArgs) -> eyre::Result<NodeArgs> {
    let Some(home) = home else {
        return Ok(given);
    };
    let file = node_config::path(home);
    if !file.exists() {
        return Ok(given);
    }
    Ok(given.over(node_config::read(&file)?))
}

fn authority(a: Option<Authority>) -> Authority {
    a.unwrap_or(Authority::Owner)
}

async fn start(home: Option<PathBuf>, args: NodeArgs) -> eyre::Result<()> {
    let node_id = args.node_id.ok_or_else(|| {
        eyre::eyre!("the node's id is not set: merod --node NAME init --node-id N ...")
    })?;
    let public_url = args
        .public_url
        .clone()
        .ok_or_else(|| eyre::eyre!("the node's public URL is not set: --public-url URL"))?;
    let data_dir = args
        .data_dir
        .clone()
        .or_else(|| home.map(|h| h.join("data")))
        .ok_or_else(|| eyre::eyre!("no data directory: --data-dir DIR (or --node NAME)"))?;
    let dev = args.dev.unwrap_or(false);
    let cluster = args.cluster.clone().unwrap_or_else(|| "dev".to_owned());
    let mut peers = BTreeMap::new();
    for p in &args.peers {
        let (id, url) = p
            .split_once('=')
            .ok_or_else(|| eyre::eyre!("peer must be id=url: {p}"))?;
        let _prev = peers.insert(id.parse()?, url.to_owned());
    }
    if args.kms_url.is_some() != args.kms_identity.is_some() {
        eyre::bail!("mero-kms needs both --kms-url and --kms-identity");
    }
    let kms_args = args.kms_url.as_deref().zip(args.kms_identity.as_deref());
    let storage_key: [u8; 32] = match kms::key_source(
        dev,
        args.storage_key.as_deref(),
        args.storage_key_file.as_deref(),
        kms_args,
    )? {
        kms::KeySource::Raw(hex_key) => hex::decode(hex_key)?
            .try_into()
            .map_err(|_| eyre::eyre!("storage key must be 32 bytes"))?,
        kms::KeySource::File(path) => kms::derive_storage_key(&kms::read_key_file(path)?, &cluster),
        kms::KeySource::Kms { url, identity } => {
            kms::derive_storage_key(&kms::fetch(url, identity).await?, &cluster)
        }
        kms::KeySource::Dev => {
            calimero_identity::sha256(format!("calimero-dev-storage-key|{cluster}").as_bytes())
        }
    };
    let peer_measurements = if dev {
        Measurements::None
    } else {
        let profiles = if args.peer_profiles.is_empty() {
            args.tee_profile.clone().into_iter().collect()
        } else {
            args.peer_profiles.clone()
        };
        if args.tee_release.is_none() || profiles.is_empty() {
            eyre::bail!(
                "a TDX node needs MERO_TEE_VERSION and MERO_TEE_PROFILE (or --peer-profiles) \
                 to judge its peers' quotes"
            );
        }
        Measurements::Release {
            allowed_profiles: profiles,
            min_version: args.peer_min_release.clone(),
        }
    };
    let outbound = match (&args.settings, &args.outbound) {
        (Some(path), _) => Some(read_settings(path)?),
        (None, o) => o.clone(),
    };
    let config = Config {
        node_id,
        public_url,
        listen: args.listen.unwrap_or_else(|| ([0, 0, 0, 0], 2528).into()),
        data_dir,
        peers,
        bootstrap: args.bootstrap.unwrap_or(false),
        cluster,
        storage_key,
        attestation: if dev { Mode::Mock } else { Mode::Tdx },
        allow_mock_peers: dev,
        sealed_required: !dev,
        rp_id: args.rp_id.clone().unwrap_or_else(|| "localhost".to_owned()),
        origins: if args.origins.is_empty() {
            vec!["http://localhost:3000".to_owned()]
        } else {
            args.origins.clone()
        },
        release: env!("CARGO_PKG_VERSION").to_owned(),
        ops_token: args.ops_token.clone(),
        ops_token_sha256: args.ops_token_sha256.clone(),
        metrics_listen: args.metrics_listen,
        replicas: args.replicas.unwrap_or(3),
        backup_dir: args.backup_dir.clone(),
        raft: calimero_log::Config::default(),
        session_key_rotation_secs: args.session_key_rotation_secs.unwrap_or(6 * 3600).max(1800),
        read_wait: std::time::Duration::from_secs(2),
        tee_release: args.tee_release.clone(),
        tee_profile: args.tee_profile.clone(),
        peer_measurements,
        clock_offset_ms: 0,
        budget: None,
        registry: args.registry_url.clone(),
        allow_unsigned_bundles: args.allow_unsigned_bundles.unwrap_or(false) || dev,
        outbound: outbound
            .map(|o| calimero_outbound::OutboundService::from_settings(&o))
            .transpose()?
            .flatten(),
        apps_domain: args
            .apps_domain
            .as_ref()
            .map(|d| d.trim_matches('.').to_ascii_lowercase()),
        governance: Governance {
            releases: authority(args.release_approvals),
            app_upgrades: authority(args.app_upgrades),
            outbound: authority(args.outbound_approvals),
        },
    };
    let running = run(config).await?;
    tokio::signal::ctrl_c().await?;
    running.stop().await;
    Ok(())
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Cli::parse();
    // A local cluster's nodes start one after another, so openraft reports the
    // ones not up yet; that is noise for a developer (RUST_LOG still overrides).
    let default_filter = if matches!(cli.cmd, Cmd::Local(_)) {
        "warn,openraft=off"
    } else {
        "info,openraft=warn"
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| default_filter.into());
    // `attest` prints its answer on stdout, so its logs go to stderr.
    if matches!(cli.cmd, Cmd::Attest(_)) {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
    let home = cli.node.as_ref().map(|n| cli.home.join(n));
    let name = cli.node.clone().unwrap_or_default();
    match cli.cmd {
        Cmd::Init { node, force } => {
            let home = need_node(home.as_ref(), "init")?;
            let file = node_config::path(&home);
            if file.exists() && !force {
                eyre::bail!(
                    "{} exists: `merod --node {name} config ...` changes it, `init --force` replaces it",
                    file.display(),
                );
            }
            if node.node_id.is_none() || node.public_url.is_none() {
                eyre::bail!("init needs at least --node-id and --public-url");
            }
            std::fs::create_dir_all(home.join("data"))?;
            node_config::write(&file, &node)?;
            println!("{}", file.display());
            Ok(())
        }
        Cmd::Config { node } => {
            let home = need_node(home.as_ref(), "config")?;
            let file = node_config::path(&home);
            if !file.exists() {
                eyre::bail!("{} does not exist: run `merod init` first", file.display());
            }
            let merged = node.over(node_config::read(&file)?);
            node_config::write(&file, &merged)?;
            print!("{}", std::fs::read_to_string(&file)?);
            Ok(())
        }
        Cmd::Run { node } => {
            let args = effective(home.as_ref(), node)?;
            start(home, args).await
        }
        Cmd::Local(args) => local(args).await,
        Cmd::Inspect(args) => {
            let data_dir = match &args.data_dir {
                Some(d) => d.clone(),
                None => {
                    let h = need_node(home.as_ref(), "inspect")?;
                    effective(home.as_ref(), NodeArgs::default())?
                        .data_dir
                        .unwrap_or_else(|| h.join("data"))
                }
            };
            inspect(&data_dir, &args)
        }
        Cmd::Kms(KmsCmd::DiskKey(cmd)) => cmd.run().await,
        Cmd::Attest(args) => attest(home.as_ref(), args).await,
    }
}
