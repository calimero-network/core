use calimero_app_downloader::registry::RegistryConfig;
use calimero_blobstore::config::BlobStoreConfig;
use calimero_config::ConfigFile;
use calimero_network_primitives::config::NetworkConfig;
use calimero_node::sync::SyncConfig;
use calimero_node::{start, NodeConfig, NodeMode, StopCause};
use calimero_server::config::{AuthMode, ServerConfig};
use calimero_store::config::StoreConfig;
use clap::Parser;
use eyre::{bail, Result as EyreResult, WrapErr};
use mero_auth::config::StorageConfig as AuthStorageConfig;
use mero_auth::embedded::default_config;
use multiaddr::{Multiaddr, Protocol};
use tracing::{error, info, warn};

use super::auth_mode::AuthModeArg;
use super::validation::validate_config;
use crate::cli::RootArgs;
use crate::kms;

/// Overrides `[registry].mode` so CI containers can select a registry without editing config.toml.
const CALIMERO_REGISTRY_MODE: &str = "CALIMERO_REGISTRY_MODE";
/// Overrides `[registry].base_url`; same rationale as `CALIMERO_REGISTRY_MODE`.
const CALIMERO_REGISTRY_URL: &str = "CALIMERO_REGISTRY_URL";

/// Run a node
#[derive(Debug, Parser)]
pub struct RunCommand {
    /// Override the authentication mode configured in config.toml
    #[arg(long, value_enum)]
    pub auth_mode: Option<AuthModeArg>,

    /// Stop when this file descriptor reaches EOF. The process that spawns merod
    /// holds the other end of a pipe, so merod exits when that process does.
    #[arg(long, value_name = "FD")]
    pub exit_on_eof: Option<i32>,

    /// Stop when stdin reaches EOF. The portable form of `--exit-on-eof`: a
    /// supervisor keeps merod's stdin open and closes it to ask for a graceful
    /// stop, and the OS closes it anyway if the supervisor dies.
    ///
    /// This is the only graceful stop Windows has. There is no `SIGTERM` there,
    /// and `taskkill` without `/F` posts `WM_CLOSE` to windows a console process
    /// does not have — so a supervisor's only other option is `/F`, which is
    /// `TerminateProcess` and runs none of the drain and flush below.
    ///
    /// Off by default: merod inherits a terminal's stdin when run by hand, and a
    /// node must not exit because someone piped it from a finished command.
    #[arg(long, default_value_t = false)]
    pub exit_on_stdin_close: bool,

    /// DEV/TEST ONLY. Produce and accept MOCK TEE attestation quotes (no real TDX).
    /// Insecure — never use in production. Refuses to start alongside a real KMS.
    /// CLI-only flag (no env inheritance); the mock path is compiled in only under
    /// the default-off `mock-attestation` build feature.
    #[clap(long, default_value_t = false)]
    pub mock_tee: bool,
}

impl RunCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        // The flag is declared unconditionally so that a binary built without
        // `mock-attestation` rejects it with an actionable message instead of a
        // bare clap "unexpected argument". Checked before anything else (node
        // init, the deny-guard below): without the feature there is no mock path
        // to guard in the first place, so the flag can never be honoured.
        #[cfg(not(feature = "mock-attestation"))]
        if self.mock_tee {
            bail!(
                "--mock-tee: this merod binary was built without mock-attestation support. \
                 Rebuild with `cargo build -p merod --features mock-attestation` (dev/test only); \
                 release binaries intentionally contain no mock attestation code."
            );
        }

        #[cfg(not(unix))]
        if self.exit_on_eof.is_some() {
            bail!("--exit-on-eof is only supported on unix");
        }

        let path = root_args.home.join(root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let mut config = ConfigFile::load(&path).await?;

        apply_registry_env(&mut config.registry, |k| std::env::var(k).ok())?;

        // Apply CLI auth_mode override before validation.
        if let Some(mode) = self.auth_mode {
            config.network.server.auth_mode = mode.into();
        }

        // Mock TEE is dev/test only and must never coexist with real attestation.
        //
        // Guard contract (deny-list, not allow-list — intentional): `--mock-tee`
        // is refused ONLY when a real KMS attestation is configured
        // (`TeeConfig::has_real_attestation`). A node with no TEE config at all,
        // or a TEE block that carries no KMS provider, is not a production
        // attestation config — so mock is allowed there, gated by the loud
        // startup warning below. Do not flip this to an allow-list; this is the
        // agreed dev-only flag behavior.
        //
        // `has_real_attestation` destructures `KmsConfig` exhaustively (no `..`),
        // so adding a second KMS provider fails to compile there until the new
        // provider is folded into the predicate — this guard cannot silently
        // stop covering a provider.
        if self.mock_tee {
            if config
                .tee
                .as_ref()
                .is_some_and(calimero_config::TeeConfig::has_real_attestation)
            {
                bail!(
                    "--mock-tee refused: a real KMS/attestation is configured. \
                     Mock TEE is dev/test only and cannot coexist with real attestation."
                );
            }
            tracing::warn!(
                "================ MOCK TEE ENABLED — INSECURE, DEV/TEST ONLY ================"
            );
            // W4: the deny-list above only refuses when `has_real_attestation()`
            // is true (Phala KMS with `attestation.enabled && !accept_mock`). A
            // node that has a Phala KMS provider configured but with
            // `enabled == false` (or `accept_mock == true`) passes the guard
            // silently — yet pairing a configured KMS provider with mock TEE is
            // almost certainly a misconfiguration (e.g. attestation was meant to
            // be on, or a prod config got `--mock-tee` by accident). Do NOT
            // refuse — that would break legitimate dev flows — but warn loudly.
            if config
                .tee
                .as_ref()
                .is_some_and(|tee| tee.kms.phala.is_some())
            {
                tracing::warn!(
                    "--mock-tee is active while a Phala KMS provider is configured \
                     (tee.kms.phala). Mock TEE bypasses real attestation; if this node \
                     is meant to use the configured KMS, this is likely a misconfiguration."
                );
            }
        }

        // Resolve external attestation policy once at startup so downstream
        // validation + key fetch paths reuse the same effective configuration.
        if let Some(tee_config) = config.tee.as_mut() {
            if let Some(phala) = tee_config.kms.phala.as_mut() {
                phala.attestation = kms::resolve_effective_attestation_config(&phala.attestation)
                    .wrap_err(
                        "Failed to resolve tee.kms.phala.attestation policy (including external policy_json_path)",
                    )?;
            }
        }

        // Validate configuration at startup (after CLI overrides are applied)
        validate_config(&config, &path).wrap_err(
            "Configuration validation failed - please fix the configuration and try again",
        )?;

        // Fetch storage encryption key from KMS if configured
        let encryption_key = if let Some(ref tee_config) = config.tee {
            let peer_id = config.identity.keypair.public().to_peer_id().to_base58();
            info!("TEE configured, fetching storage key for peer {}", peer_id);

            let policy = crate::kms_policy::resolve_policy().await?;
            let key = kms::fetch_storage_key(
                &tee_config.kms,
                &peer_id,
                &config.identity.keypair,
                policy.as_ref(),
            )
            .await
            .wrap_err(
                "TEE storage encryption is configured but failed to fetch key from KMS. \
                     The node cannot start without the encryption key to prevent unencrypted data storage.",
            )?;

            info!(
                "Storage encryption key fetched successfully (key_len={})",
                key.len()
            );
            Some(key)
        } else {
            info!("TEE not configured; starting without KMS key-fetch flow");
            None
        };

        // Read node mode from config
        let node_mode = config.mode;

        // In read-only mode, disable JSON-RPC to prevent execution requests
        if node_mode == NodeMode::ReadOnly {
            info!("Starting node in read-only mode - JSON-RPC execution disabled");
            config.network.server.jsonrpc = None;
        }

        let network = config.network;
        let mut server_source = network.server;

        // Ensure embedded_auth config exists with resolved paths when embedded mode is active
        if matches!(server_source.auth_mode, AuthMode::Embedded) {
            let mut auth_config = server_source
                .embedded_auth
                .take()
                .unwrap_or_else(default_config);

            // Resolve relative RocksDB paths against the node's home directory
            if let AuthStorageConfig::RocksDB { path: storage_path } = &mut auth_config.storage {
                *storage_path = crate::cli::resolve_node_relative_path(
                    path.as_std_path(),
                    storage_path.clone(),
                );
            }

            server_source.embedded_auth = Some(auth_config);
        } else if let Some(cfg) = server_source.embedded_auth.as_mut() {
            // Also resolve paths for proxy mode if config exists
            if let AuthStorageConfig::RocksDB { path: storage_path } = &mut cfg.storage {
                *storage_path = crate::cli::resolve_node_relative_path(
                    path.as_std_path(),
                    storage_path.clone(),
                );
            }
        }
        if let Some(msg) =
            unauthenticated_exposure_warning(server_source.auth_mode, &server_source.listen)
        {
            warn!("{msg}");
        }

        let server_config = ServerConfig::with_auth(
            server_source.listen,
            config.identity.keypair.clone(),
            calimero_server::config::ServiceConfigs {
                admin: server_source.admin,
                jsonrpc: server_source.jsonrpc,
                websocket: server_source.websocket,
                sse: server_source.sse,
            },
            server_source.auth_mode,
            server_source.embedded_auth,
        );

        // Create store config with optional encryption
        let datastore_path = path.join(config.datastore.path);
        let datastore_config = match encryption_key {
            Some(key) => {
                info!("Storage encryption enabled");
                // Move the fetched key into a `Zeroizing` wrapper so the KEK is
                // wiped from the heap when the config drops rather than
                // lingering for the whole process lifetime.
                StoreConfig::with_encryption(datastore_path, zeroize::Zeroizing::new(key))
            }
            None => StoreConfig::new(datastore_path),
        };

        #[cfg(unix)]
        let stop_watch = {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let data_dir = datastore_config.path.clone().into_std_path_buf();
            let parent_fd = self.exit_on_eof;
            let stdin_close = self.exit_on_stdin_close;
            drop(tokio::spawn(async move {
                let cause = tokio::select! {
                    detail = crate::watchdog::parent_closed(parent_fd) => {
                        warn!("{detail}; shutting down");
                        StopCause::ParentExited
                    }
                    detail = crate::watchdog::stdin_closed(stdin_close) => {
                        warn!("{detail}; shutting down");
                        StopCause::ParentExited
                    }
                    detail = crate::watchdog::data_dir_replaced(
                        data_dir,
                        crate::watchdog::DATA_DIR_CHECK_INTERVAL,
                    ) => {
                        error!("{detail}; shutting down without writing anything further");
                        StopCause::DataDirReplaced
                    }
                };
                let _ = tx.send(cause);
            }));
            Some(rx)
        };
        // Off unix this is the whole watchdog. `parent_closed` needs an inherited
        // fd and `data_dir_replaced` identifies a directory by `(dev, ino)`, so
        // both stay unix-only; stdin EOF needs neither.
        #[cfg(not(unix))]
        let stop_watch = if self.exit_on_stdin_close {
            let (tx, rx) = tokio::sync::oneshot::channel();
            drop(tokio::spawn(async move {
                let detail = crate::watchdog::stdin_closed(true).await;
                warn!("{detail}; shutting down");
                let _ = tx.send(StopCause::ParentExited);
            }));
            Some(rx)
        } else {
            None
        };

        start(NodeConfig {
            home: path.clone(),
            identity: config.identity.keypair.clone(),
            network: NetworkConfig::new(
                config.identity.keypair.clone(),
                network.swarm,
                network.bootstrap,
                network.discovery,
            ),
            sync: SyncConfig {
                timeout: config.sync.timeout,
                session_deadline: config.sync.session_deadline,
                interval: config.sync.interval,
                frequency: config.sync.frequency,
                ..Default::default() // Use defaults for new fields
            },
            datastore: datastore_config,
            blobstore: BlobStoreConfig::new(path.join(config.blobstore.path)),
            context: config.context,
            registry: config.registry,
            server: server_config,
            gc_interval_secs: None, // Use default (12 hours)
            dag_compaction: config.dag_compaction,
            mode: node_mode,
            stop_watch,
            vm_limits: config.runtime.vm_limits(),
            #[cfg(feature = "mock-attestation")]
            mock_tee: self.mock_tee,
        })
        .await
    }
}

/// Applies `CALIMERO_REGISTRY_MODE`/`CALIMERO_REGISTRY_URL` over a loaded config.
/// A malformed value errors instead of keeping config.toml's setting - a misconfigured node must not start.
fn apply_registry_env(
    cfg: &mut RegistryConfig,
    get: impl Fn(&str) -> Option<String>,
) -> EyreResult<()> {
    if let Some(raw) = get(CALIMERO_REGISTRY_MODE) {
        cfg.mode = raw
            .parse()
            .map_err(|e: String| eyre::eyre!("{CALIMERO_REGISTRY_MODE} is invalid: {e}"))?;
    }
    if let Some(raw) = get(CALIMERO_REGISTRY_URL) {
        cfg.base_url = Some(
            raw.parse()
                .map_err(|e| eyre::eyre!("{CALIMERO_REGISTRY_URL} is invalid: {e}"))?,
        );
    }
    Ok(())
}

/// Warn when the server is reachable off-box but the node authenticates nothing
/// itself. In `Proxy` mode the node relies entirely on a front reverse proxy for
/// auth, so a non-loopback bind without one exposes an unauthenticated admin/RPC
/// API. Returns the warning message when it applies, else `None`.
fn unauthenticated_exposure_warning(auth_mode: AuthMode, listen: &[Multiaddr]) -> Option<String> {
    if !matches!(auth_mode, AuthMode::Proxy) {
        return None;
    }

    let exposed: Vec<&Multiaddr> = listen
        .iter()
        .filter(|addr| multiaddr_ip(addr).is_some_and(|ip| !ip.is_loopback()))
        .collect();

    if exposed.is_empty() {
        return None;
    }

    let addrs = exposed
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "SECURITY: server is bound to non-loopback address(es) [{addrs}] while auth mode is \
         Proxy - the node performs NO authentication of its own in Proxy mode. Ensure an \
         authenticating reverse proxy is in front of it, or switch to Embedded auth. The \
         admin/RPC API is otherwise reachable UNAUTHENTICATED from the network."
    ))
}

fn multiaddr_ip(addr: &Multiaddr) -> Option<core::net::IpAddr> {
    addr.iter().find_map(|proto| match proto {
        Protocol::Ip4(ip) => Some(ip.into()),
        Protocol::Ip6(ip) => Some(ip.into()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use calimero_app_downloader::registry::RegistryMode;

    use super::*;

    fn addr(s: &str) -> Multiaddr {
        s.parse().unwrap()
    }

    #[test]
    fn apply_registry_env_overrides_mode_and_url() {
        let mut cfg = RegistryConfig::default();
        apply_registry_env(&mut cfg, |k| match k {
            "CALIMERO_REGISTRY_MODE" => Some("dht".to_owned()),
            "CALIMERO_REGISTRY_URL" => Some("https://reg.example".to_owned()),
            _ => None,
        })
        .expect("valid overrides must apply");
        assert_eq!(cfg.mode, RegistryMode::Dht);
        assert_eq!(
            cfg.base_url.map(String::from),
            Some("https://reg.example/".to_owned())
        );
    }

    #[test]
    fn apply_registry_env_leaves_config_when_absent() {
        let mut cfg = RegistryConfig::new(
            RegistryMode::Http,
            Some("https://apps.calimero.network".parse().unwrap()),
        );
        let before = cfg.clone();
        apply_registry_env(&mut cfg, |_| None).expect("absent env must not error");
        assert_eq!(cfg.mode, before.mode);
        assert_eq!(cfg.base_url, before.base_url);
    }

    #[test]
    fn apply_registry_env_rejects_garbage_mode() {
        let mut cfg = RegistryConfig::default();
        let err = apply_registry_env(&mut cfg, |k| {
            (k == "CALIMERO_REGISTRY_MODE").then(|| "carrier-pigeon".to_owned())
        })
        .expect_err("an unknown mode must not start the node");
        assert!(err.to_string().contains("CALIMERO_REGISTRY_MODE"));
    }

    #[test]
    fn apply_registry_env_rejects_garbage_url() {
        let mut cfg = RegistryConfig::default();
        let err = apply_registry_env(&mut cfg, |k| {
            (k == "CALIMERO_REGISTRY_URL").then(|| "not a url".to_owned())
        })
        .expect_err("an unparseable URL must not start the node");
        assert!(err.to_string().contains("CALIMERO_REGISTRY_URL"));
    }

    #[test]
    fn proxy_non_loopback_warns() {
        let listen = vec![addr("/ip4/0.0.0.0/tcp/2528")];
        assert!(unauthenticated_exposure_warning(AuthMode::Proxy, &listen).is_some());
    }

    #[test]
    fn proxy_loopback_only_is_silent() {
        let listen = vec![addr("/ip4/127.0.0.1/tcp/2528"), addr("/ip6/::1/tcp/2528")];
        assert!(unauthenticated_exposure_warning(AuthMode::Proxy, &listen).is_none());
    }

    #[test]
    fn embedded_non_loopback_is_silent() {
        let listen = vec![addr("/ip4/0.0.0.0/tcp/2528")];
        assert!(unauthenticated_exposure_warning(AuthMode::Embedded, &listen).is_none());
    }

    #[test]
    fn proxy_mixed_loopback_and_public_warns() {
        let listen = vec![
            addr("/ip4/127.0.0.1/tcp/2528"),
            addr("/ip4/192.168.1.5/tcp/2528"),
        ];
        let msg = unauthenticated_exposure_warning(AuthMode::Proxy, &listen).unwrap();
        assert!(msg.contains("192.168.1.5"));
        assert!(!msg.contains("127.0.0.1"));
    }
}
