//! `merod-v2`: the Calimero cloud node.

use std::collections::BTreeMap;
use std::path::PathBuf;

use calimero_cloud_node::attest::Mode;
use calimero_cloud_node::{run, Config};
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
    /// Storage key (hex). In production it comes from mero-kms after attestation.
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
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,openraft=warn".into()),
        )
        .init();
    let args = Args::parse();
    let mut peers = BTreeMap::new();
    for p in &args.peers {
        let (id, url) = p
            .split_once('=')
            .ok_or_else(|| eyre::eyre!("peer must be id=url: {p}"))?;
        let _prev = peers.insert(id.parse()?, url.to_owned());
    }
    let storage_key: [u8; 32] = match (&args.storage_key, args.dev) {
        (Some(hex_key), _) => hex::decode(hex_key)?.try_into().map_err(|_| eyre::eyre!("storage key must be 32 bytes"))?,
        (None, true) => calimero_identity::sha256(format!("calimero-dev-storage-key|{}", args.cluster).as_bytes()),
        (None, false) => eyre::bail!("no storage key: production nodes fetch it from mero-kms (not wired in this build); pass --storage-key or --dev"),
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
    };
    let running = run(config).await?;
    tokio::signal::ctrl_c().await?;
    running.stop().await;
    Ok(())
}
