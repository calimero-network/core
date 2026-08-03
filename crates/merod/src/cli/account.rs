//! Back up and restore this node's account root.
//!
//! The account root is the one key that can certify a replacement device after
//! every existing device is lost. It is generated on first use and written to the
//! node's own store, so until this command existed the recovery property the
//! account model is built around was structurally available but not deliverable:
//! the key survives a namespace-identity rotation and does not survive losing the
//! disk, which is the case the whole story is about.
//!
//! A backup is **one secret plus a non-secret list**. The per-namespace nonce is
//! derived (`KDF(root_secret, namespace_id)`), so the words below recover every
//! account this node owns in every namespace it ever joined — provided the
//! operator still knows which namespaces those were. `export` prints the ids it
//! can derive for any namespace named on the command line, precisely so that list
//! can be kept somewhere ordinary.
//!
//! Both subcommands open the node's store directly, so the node must be **stopped**
//! — RocksDB holds an exclusive lock while `merod run` is up.

use calimero_config::ConfigFile;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{AccountRoot, NodeDeviceRepository};
use calimero_store::config::StoreConfig;
use calimero_store::Store;
use calimero_store_rocksdb::RocksDB;
use clap::{Parser, Subcommand};
use eyre::{bail, Result as EyreResult, WrapErr};
use zeroize::Zeroizing;

use crate::cli::RootArgs;

#[derive(Debug, Parser)]
pub struct AccountCommand {
    #[command(subcommand)]
    action: AccountSubcommands,
}

#[derive(Debug, Subcommand)]
enum AccountSubcommands {
    /// Print this node's account root as a 24-word recovery phrase
    Export(ExportCommand),
    /// Restore an account root from a recovery phrase
    Import(ImportCommand),
}

#[derive(Debug, Parser)]
pub struct ExportCommand {
    /// Also print the account id this root owns in NAMESPACE_ID (hex, repeatable).
    ///
    /// The non-secret half of a backup: the phrase recovers the key, this tells you
    /// which accounts to expect back.
    #[arg(long = "namespace", value_name = "NAMESPACE_ID")]
    namespaces: Vec<String>,

    /// Write the phrase to PATH instead of stdout. Requires `--allow-plaintext-file`.
    #[arg(long, value_name = "PATH")]
    out: Option<camino::Utf8PathBuf>,

    /// Acknowledge that `--out` writes the recovery key to disk in the clear.
    #[arg(long, default_value_t = false)]
    allow_plaintext_file: bool,
}

#[derive(Debug, Parser)]
pub struct ImportCommand {
    /// Read the phrase from PATH instead of stdin.
    #[arg(long, value_name = "PATH")]
    from: Option<camino::Utf8PathBuf>,

    /// Replace an existing root. Unrecoverable — see the warning this prints.
    #[arg(long, default_value_t = false)]
    force: bool,
}

impl AccountCommand {
    pub async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        match self.action {
            AccountSubcommands::Export(cmd) => cmd.run(root_args).await,
            AccountSubcommands::Import(cmd) => cmd.run(root_args).await,
        }
    }
}

/// Open the node's datastore for a direct read/write, outside a running node.
///
/// Refuses an encrypted store rather than reading it as garbage: the key lives in
/// KMS and is fetched during `run` with the node's TEE identity, which a recovery
/// CLI on a replacement machine generally cannot reproduce. Saying so is more
/// useful than failing to decode a row.
async fn open_store(root_args: &RootArgs) -> EyreResult<Store> {
    let path = root_args.home.join(&root_args.node_name);
    if !ConfigFile::exists(&path) {
        bail!("Node is not initialized in {path:?}");
    }
    let config = ConfigFile::load(&path)
        .await
        .wrap_err("Failed to load node configuration")?;

    if config.tee.is_some() {
        bail!(
            "This node's datastore is encrypted with a KMS-held key, which this \
             command cannot fetch. Export the root from a node that can reach the \
             KMS, or read it through the running node."
        );
    }

    let datastore_path = path.join(config.datastore.path);
    Store::open::<RocksDB>(&StoreConfig::new(datastore_path)).wrap_err(
        "Failed to open the datastore. If the node is running, stop it first — \
         RocksDB holds an exclusive lock.",
    )
}

impl ExportCommand {
    async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        if self.out.is_some() && !self.allow_plaintext_file {
            bail!(
                "Refusing to write the recovery key to a file in the clear. It is \
                 the whole account: anyone holding these words can certify a device \
                 into every account this node owns. Prefer copying it from stdout \
                 onto paper or into a password manager; pass \
                 --allow-plaintext-file if you have somewhere safe for it."
            );
        }

        let store = open_store(root_args).await?;
        let repo = NodeDeviceRepository::new(&store);
        // Deliberately NOT `ensure_account_root`: generating one here would report
        // a brand-new key as this node's backup, and the operator would keep it as
        // if it meant something.
        let Some(root) = repo
            .account_root()
            .wrap_err("Failed to read the account root")?
        else {
            bail!(
                "This node has no account root yet — nothing to export. One is \
                 generated the first time an account is used here (`meroctl account \
                 create <NAMESPACE_ID>`)."
            );
        };

        let phrase = root.to_mnemonic()?;

        let mut ids = Vec::with_capacity(self.namespaces.len());
        for raw in &self.namespaces {
            let namespace = parse_namespace(raw)?;
            ids.push((raw.clone(), root.account_for(&namespace)));
        }

        if let Some(path) = self.out {
            write_owner_only(&path, &format!("{}\n", phrase.as_str()))
                .wrap_err_with(|| format!("Failed to write the recovery phrase to {path}"))?;
            println!("Recovery phrase written to {path} (owner-only, 0600)");
            println!("It is unencrypted. Move it somewhere safe and delete this copy.");
        } else {
            println!("{}", phrase.as_str());
        }

        println!();
        println!("Account root public key: {}", root.public_key());
        for (raw, id) in ids {
            println!("  namespace {raw} -> account {id}");
        }
        println!();
        println!(
            "Keep the phrase AND the list of namespaces you use it in. The phrase \
             recovers the key; the list is what tells you which accounts to expect \
             back, and it is not a secret."
        );

        Ok(())
    }
}

impl ImportCommand {
    async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        // `Zeroizing` so the words are wiped when this scope ends, matching what
        // `to_mnemonic` hands back on the export side. It covers the buffer we
        // own, not every copy: `read_to_string` allocates and grows its own
        // internally, and a shell that echoed the paste has it too. Wiping ours
        // is still worth doing — it is the copy that lives longest.
        let phrase: Zeroizing<String> = match &self.from {
            Some(path) => Zeroizing::new(
                std::fs::read_to_string(path)
                    .wrap_err_with(|| format!("Failed to read the recovery phrase from {path}"))?,
            ),
            None => {
                eprintln!("Paste the 24-word recovery phrase, then press Ctrl-D:");
                let mut buf = Zeroizing::new(String::new());
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                    .wrap_err("Failed to read the recovery phrase from stdin")?;
                buf
            }
        };

        // Parse BEFORE touching the store: a typo must not be able to leave the
        // node in a half-imported state, and the checksum is the whole reason the
        // backup is words rather than hex.
        let root = AccountRoot::from_mnemonic(&phrase)?;

        let store = open_store(root_args).await?;
        let repo = NodeDeviceRepository::new(&store);

        // The repository owns the refusal, so no caller can skip it by forgetting
        // to check first. `--force` only decides what this one asks for.
        let outcome = repo.try_import_account_root(&root, self.force).wrap_err(
            "Failed to import the account root. If one already exists, export it \
             first and then pass --force to replace it.",
        )?;

        if let Some(previous) = &outcome.replaced {
            eprintln!(
                "Replaced the account root {}. Every account it owned is now \
                 reachable only from ITS phrase, not this one.",
                previous.public_key()
            );
        }

        println!("Imported account root {}", root.public_key());

        // Report what the store actually holds rather than asserting the clean
        // case. On a wiped node there are no rows and "none yet" is true; on a
        // forced import over a live node it is exactly wrong, and that is the
        // situation where an operator most needs to know which namespaces just lost
        // their device.
        if outcome.released.is_empty() && outcome.retained.is_empty() {
            println!(
                "Devices are not restored: this node holds none in any namespace yet. \
                 Enrol one per namespace (`meroctl account create <NAMESPACE_ID>`) and \
                 a peer will deliver the current scope key."
            );
        } else {
            if !outcome.released.is_empty() {
                println!();
                println!(
                    "Dropped {} device(s) that belonged to the replaced root — their ids \
                     are spent, and each namespace needs a fresh enrolment:",
                    outcome.released.len()
                );
                for namespace in &outcome.released {
                    println!("  {namespace:?}");
                }
                println!(
                    "Enrol again per namespace (`meroctl account create <NAMESPACE_ID>`); \
                     a peer delivers the current scope key to the new device."
                );
            }
            if !outcome.retained.is_empty() {
                println!();
                println!(
                    "Kept {} device(s) paired into an account this root does not own — \
                     replacing this node's root does not affect them:",
                    outcome.retained.len()
                );
                for namespace in &outcome.retained {
                    println!("  {namespace:?}");
                }
            }
        }

        Ok(())
    }
}

/// Write `contents` to `path`, owner-readable only.
///
/// Created with mode `0600` rather than written and then chmod-ed, because the
/// gap between the two is exactly long enough for another local user to read a
/// recovery key — and this file *is* the account. `set_permissions` afterwards
/// covers the other case: `.mode()` applies only when the file is created, so an
/// existing world-readable file at this path would otherwise keep its mode.
///
/// The node home gets the same treatment at `init` (`restrict_to_owner`), which
/// chmods after the fact; that is fine for a directory tree being built, and not
/// for a single secret written on demand.
#[cfg(unix)]
fn write_owner_only(path: &camino::Utf8Path, contents: &str) -> EyreResult<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Non-Unix fallback: no mode bits to set, so this is a plain write.
#[cfg(not(unix))]
fn write_owner_only(path: &camino::Utf8Path, contents: &str) -> EyreResult<()> {
    std::fs::write(path, contents)?;
    Ok(())
}

/// Parse a hex namespace id, with a message that says which argument was wrong —
/// `export` takes several and a bare "invalid hex" would not say which.
fn parse_namespace(raw: &str) -> EyreResult<ContextGroupId> {
    let bytes =
        hex::decode(raw.trim()).wrap_err_with(|| format!("namespace '{raw}' is not hex"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| eyre::eyre!("namespace '{raw}' is not 32 bytes (64 hex characters)"))?;
    Ok(ContextGroupId::from(bytes))
}
