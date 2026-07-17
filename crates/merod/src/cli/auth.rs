use calimero_config::ConfigFile;
use clap::{Parser, Subcommand};
use eyre::{bail, Result as EyreResult};
use mero_auth::config::StorageConfig as AuthStorageConfig;
use mero_auth::provisioning;
use mero_auth::storage::create_storage;
use tracing::info;

use super::admin_creds::AdminCredArgs;
use crate::cli::RootArgs;

/// Manage embedded-auth accounts of an initialized node
#[derive(Debug, Parser)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub action: AuthSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum AuthSubCommands {
    SetAdmin(SetAdminCommand),
}

/// Mint (or re-mint) the admin root key directly in the node's auth storage.
///
/// Offline recovery and migration path: run it while the node is stopped to
/// provision the admin account on a node initialized without one (e.g. with
/// `--no-admin`, or by a release predating credentials-at-init), or to
/// regain access after the auth storage was lost. Requires filesystem access
/// to the node home — the same trust boundary as the node's private key.
///
/// Re-minting with new credentials adds a new admin root key; it does not
/// remove keys minted under other credentials.
#[derive(Debug, Parser)]
pub struct SetAdminCommand {
    #[clap(flatten)]
    pub admin: AdminCredArgs,
}

impl AuthCommand {
    pub async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        match self.action {
            AuthSubCommands::SetAdmin(cmd) => cmd.run(root_args).await,
        }
    }
}

impl SetAdminCommand {
    pub async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config = ConfigFile::load(&path).await?;

        let Some(auth_config) = config.network.server.embedded_auth else {
            bail!(
                "this node has no embedded_auth configuration (auth mode: proxy?); \
                 `auth set-admin` only applies to embedded auth"
            );
        };

        let auth_db_path = match auth_config.storage {
            AuthStorageConfig::RocksDB { path: storage_path } => {
                if storage_path.is_relative() {
                    path.as_std_path().join(storage_path)
                } else {
                    storage_path
                }
            }
            AuthStorageConfig::Memory => bail!(
                "this node uses in-memory auth storage, which holds no persistent \
                 accounts; set {} and {} when running the node instead",
                provisioning::ADMIN_USER_ENV,
                provisioning::ADMIN_PASSWORD_ENV,
            ),
        };

        let Some((username, password)) = self.admin.resolve()? else {
            bail!(
                "admin credentials required: pass --admin-user with \
                 --admin-password-file or --admin-password-stdin, or set {} and {}",
                provisioning::ADMIN_USER_ENV,
                provisioning::ADMIN_PASSWORD_ENV,
            );
        };

        // RocksDB takes an exclusive lock, so this fails while the node is
        // up — which is exactly the contract: set-admin is an offline tool.
        let auth_storage = create_storage(&AuthStorageConfig::RocksDB {
            path: auth_db_path.clone(),
        })
        .await
        .map_err(|err| {
            eyre::eyre!(
                "failed to open auth storage at {auth_db_path:?} (is the node still \
                 running? stop it first): {err}"
            )
        })?;

        let _key_id = provisioning::provision_admin_key(
            &auth_storage,
            &auth_config.user_password,
            &username,
            &password,
        )
        .await?;

        info!("Admin account provisioned (user: {username}); start the node and log in");

        Ok(())
    }
}
