//! `merod-v2`: the Calimero cloud node.

use std::collections::BTreeMap;
use std::path::PathBuf;

use calimero_cloud_client::Measurements;
use calimero_cloud_node::attest::Mode;
use calimero_cloud_node::{kms, run, Config};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "merod-v2",
    about = "Calimero cloud node: attested, one Raft log per app"
)]
struct Args {
    /// This node's id in the cluster.
    #[arg(long, env = "MEROD_NODE_ID")]
    node_id: u64,
    /// Address to listen on.
    #[arg(long, env = "MEROD_LISTEN", default_value = "0.0.0.0:2528")]
    listen: std::net::SocketAddr,
    /// Base URL peers and clients use for this node.
    #[arg(long, env = "MEROD_PUBLIC_URL")]
    public_url: String,
    #[arg(long, env = "MEROD_DATA_DIR")]
    data_dir: PathBuf,
    /// Cluster members as `id=url`, comma separated (this node included).
    #[arg(long, env = "MEROD_PEERS", value_delimiter = ',')]
    peers: Vec<String>,
    /// Form the system group with `--peers` as voters (first start).
    #[arg(long, env = "MEROD_BOOTSTRAP")]
    bootstrap: bool,
    #[arg(long, env = "MEROD_CLUSTER", default_value = "dev")]
    cluster: String,
    /// Development mode: mock quotes, a dev storage key, plain requests allowed.
    #[arg(long, env = "MEROD_DEV")]
    dev: bool,
    /// Storage key (hex; development only). In production it comes from mero-kms after attestation.
    #[arg(long, env = "MEROD_STORAGE_KEY")]
    storage_key: Option<String>,
    #[arg(long, env = "MEROD_RP_ID", default_value = "localhost")]
    rp_id: String,
    #[arg(
        long,
        env = "MEROD_ORIGINS",
        value_delimiter = ',',
        default_value = "http://localhost:3000"
    )]
    origins: Vec<String>,
    #[arg(long, env = "MEROD_OPS_TOKEN")]
    ops_token: Option<String>,
    #[arg(long, env = "MEROD_REPLICAS", default_value_t = 3)]
    replicas: usize,
    #[arg(long, env = "MEROD_BACKUP_DIR")]
    backup_dir: Option<PathBuf>,
    /// Rotate the session-signing key after this many seconds (at least 1800:
    /// the previous key must outlive the 15-minute sessions it signed).
    #[arg(long, env = "MEROD_SESSION_KEY_ROTATION", default_value_t = 6 * 3600)]
    session_key_rotation_secs: u64,
    /// A file holding the storage key material (development only).
    #[arg(long, env = "MEROD_STORAGE_KEY_FILE", conflicts_with_all = ["storage_key", "kms_url"])]
    storage_key_file: Option<PathBuf>,
    /// Fetch the storage key from mero-kms (needs `--kms-identity`); the only source in TDX mode.
    #[arg(
        long,
        env = "MEROD_KMS_URL",
        requires = "kms_identity",
        conflicts_with = "storage_key"
    )]
    kms_url: Option<String>,
    /// The cluster's disk identity (shared by its nodes, so they derive one key).
    #[arg(long, env = "MEROD_KMS_IDENTITY")]
    kms_identity: Option<PathBuf>,
    /// The mero-tee release this image is (names the signed measurements).
    #[arg(long, env = "MERO_TEE_VERSION")]
    tee_release: Option<String>,
    /// The mero-tee image profile this node runs.
    #[arg(long, env = "MERO_TEE_PROFILE")]
    tee_profile: Option<String>,
    /// Profiles a peer's quote may match (default: this node's own profile).
    #[arg(long, env = "MEROD_PEER_PROFILES", value_delimiter = ',')]
    peer_profiles: Vec<String>,
    /// The oldest mero-tee release a peer may run.
    #[arg(long, env = "MEROD_PEER_MIN_RELEASE")]
    peer_min_release: Option<String>,
    /// The package registry nodes fetch signed releases from.
    #[arg(long, env = "MEROD_REGISTRY_URL")]
    registry_url: Option<String>,
    /// Accept raw WASM uploads, which carry no publisher signature (default: only with --dev).
    #[arg(long, env = "MEROD_ALLOW_UNSIGNED_BUNDLES")]
    allow_unsigned_bundles: bool,
    /// A TOML settings file; its `[outbound]` table is the outbound HTTPS
    /// policy (hosts, secrets). Without it, apps cannot call out.
    #[arg(long, env = "MEROD_SETTINGS")]
    settings: Option<PathBuf>,
}

/// `--settings`: what the operator configures beyond the flags.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    outbound: calimero_outbound::OutboundSettings,
}

fn outbound(
    path: Option<&std::path::Path>,
) -> eyre::Result<Option<std::sync::Arc<calimero_outbound::OutboundService>>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(path)
        .map_err(|e| eyre::eyre!("reading {}: {e}", path.display()))?;
    let settings: Settings =
        toml::from_str(&text).map_err(|e| eyre::eyre!("{}: {e}", path.display()))?;
    calimero_outbound::OutboundService::from_settings(&settings.outbound)
}

/// `merod-v2 local`: a development cluster in this process.
#[derive(Debug, Parser)]
#[command(name = "merod-v2 local")]
struct LocalArgs {
    /// How many nodes.
    #[arg(long, default_value_t = 3)]
    nodes: u64,
    /// Node i listens on base-port + i.
    #[arg(long, default_value_t = 2500)]
    base_port: u16,
    /// Where the cluster keeps its state (kept across restarts).
    #[arg(long, default_value = ".merod-v2-local")]
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

/// `merod-v2 kms disk-key`: fetch the data-disk key (for the boot script).
#[derive(Debug, Parser)]
#[command(name = "merod-v2 kms")]
enum KmsArgs {
    DiskKey(calimero_kms_client::disk_key::KmsDiskKeyCommand),
}

async fn local(args: LocalArgs) -> eyre::Result<()> {
    if args.fresh && args.dir.exists() {
        std::fs::remove_dir_all(&args.dir)?;
    }
    let token = args
        .ops_token
        .unwrap_or_else(|| hex::encode(calimero_identity::random::<16>()));
    let cluster = calimero_cloud_node::local::start(
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
        calimero_cloud_node::local::CLUSTER
    );
    for (i, url) in cluster.urls.iter().enumerate() {
        println!("  node {}  {url}", i + 1);
    }
    println!("  state     {}", args.dir.display());
    println!("  ops token {token}");
    println!();
    println!(
        "  calimero-cloud --url {} --allow-mock login --dev-passkey ./dev-passkey.json",
        cluster.urls[0]
    );
    tokio::signal::ctrl_c().await?;
    cluster.stop().await;
    Ok(())
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let is_local = std::env::args().nth(1).as_deref() == Some("local");
    // A local cluster's nodes start one after another, so openraft reports the
    // ones not up yet; that is noise for a developer (RUST_LOG still overrides).
    let default_filter = if is_local {
        "warn,openraft=off"
    } else {
        "info,openraft=warn"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .init();
    if is_local {
        return local(LocalArgs::parse_from(std::env::args().skip(1))).await;
    }
    if std::env::args().nth(1).as_deref() == Some("kms") {
        let KmsArgs::DiskKey(cmd) = KmsArgs::parse_from(std::env::args().skip(1));
        return cmd.run().await;
    }
    let args = Args::parse();
    let mut peers = BTreeMap::new();
    for p in &args.peers {
        let (id, url) = p
            .split_once('=')
            .ok_or_else(|| eyre::eyre!("peer must be id=url: {p}"))?;
        let _prev = peers.insert(id.parse()?, url.to_owned());
    }
    let kms_args = args.kms_url.as_deref().zip(args.kms_identity.as_deref());
    let storage_key: [u8; 32] = match kms::key_source(
        args.dev,
        args.storage_key.as_deref(),
        args.storage_key_file.as_deref(),
        kms_args,
    )? {
        kms::KeySource::Raw(hex_key) => hex::decode(hex_key)?
            .try_into()
            .map_err(|_| eyre::eyre!("storage key must be 32 bytes"))?,
        kms::KeySource::File(path) => {
            kms::derive_storage_key(&kms::read_key_file(path)?, &args.cluster)
        }
        kms::KeySource::Kms { url, identity } => {
            kms::derive_storage_key(&kms::fetch(url, identity).await?, &args.cluster)
        }
        kms::KeySource::Dev => calimero_identity::sha256(
            format!("calimero-dev-storage-key|{}", args.cluster).as_bytes(),
        ),
    };
    let peer_measurements = if args.dev {
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
    let config = Config {
        node_id: args.node_id,
        public_url: args.public_url,
        listen: args.listen,
        data_dir: args.data_dir,
        peers,
        bootstrap: args.bootstrap,
        cluster: args.cluster,
        storage_key,
        attestation: if args.dev { Mode::Mock } else { Mode::Tdx },
        allow_mock_peers: args.dev,
        sealed_required: !args.dev,
        rp_id: args.rp_id,
        origins: args.origins,
        release: env!("CARGO_PKG_VERSION").to_owned(),
        ops_token: args.ops_token,
        replicas: args.replicas,
        backup_dir: args.backup_dir,
        raft: calimero_log::Config::default(),
        session_key_rotation_secs: args.session_key_rotation_secs.max(1800),
        tee_release: args.tee_release,
        tee_profile: args.tee_profile,
        peer_measurements,
        registry: args.registry_url,
        allow_unsigned_bundles: args.allow_unsigned_bundles || args.dev,
        outbound: outbound(args.settings.as_deref())?,
    };
    let running = run(config).await?;
    tokio::signal::ctrl_c().await?;
    running.stop().await;
    Ok(())
}
