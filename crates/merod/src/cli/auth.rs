use std::path::Path;

use calimero_config::ConfigFile;
use clap::{Parser, Subcommand};
use eyre::{bail, Result as EyreResult};
use mero_auth::config::{StorageConfig as AuthStorageConfig, UserPasswordConfig};
use mero_auth::provisioning;
use mero_auth::storage::create_storage;
use tracing::info;

use super::admin_creds::AdminCredArgs;
use crate::cli::RootArgs;

/// Open the auth RocksDB at `auth_db_path`, mint the admin root key, close
/// the store, and pin the database tree to owner-only (same defense in depth
/// as the datastore). Shared by `merod init` and `merod auth set-admin` so
/// the two provisioning paths cannot drift.
pub(crate) async fn provision_admin_into_storage(
    auth_db_path: &Path,
    policy: &UserPasswordConfig,
    username: &str,
    password: &str,
) -> EyreResult<()> {
    // RocksDB takes an exclusive lock, so this fails while a node holds the
    // database open — for `set-admin` that is exactly the offline contract.
    let auth_storage = create_storage(&AuthStorageConfig::RocksDB {
        path: auth_db_path.to_path_buf(),
    })
    .await
    .map_err(|err| {
        eyre::eyre!(
            "failed to open auth storage at {auth_db_path:?} (is the node still \
             running? stop it first): {err}"
        )
    })?;

    let _key_id =
        provisioning::provision_admin_key(&auth_storage, policy, username, password).await?;
    drop(auth_storage);

    super::init::restrict_tree_to_owner(auth_db_path).await
}

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
/// Re-minting with the SAME username rotates the account: the previous
/// password's key is deleted and stops authenticating. A different username
/// adds a separate admin account and leaves existing ones untouched.
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
                crate::cli::resolve_node_relative_path(path.as_std_path(), storage_path)
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

        provision_admin_into_storage(
            &auth_db_path,
            &auth_config.user_password,
            &username,
            &password,
        )
        .await?;

        info!("Admin account provisioned (user: {username}); start the node and log in");

        Ok(())
    }
}
