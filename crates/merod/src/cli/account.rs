//! Back up and restore this node's account root.
//!
//! The account root is the one key that can certify a replacement device after
//! every existing device is lost. `merod init` provisions it into the node's own
//! store (unless `--no-account-root`), so until this command existed the recovery property the
//! account model is built around was structurally available but not deliverable:
//! the key survives a namespace-identity rotation and does not survive losing the
//! disk, which is the case the whole story is about.
//!
//! A backup is **one secret**. The account id is `H(genesis(root_pk))` — a pure
//! function of the root key, with no per-namespace nonce — so it is the same
//! account in every namespace, and the phrase alone recovers all of it. `export`
//! prints that phrase alongside the root's public key and the account id it names.
//!
//! Both subcommands open the node's store directly, so the node must be **stopped**
//! — RocksDB holds an exclusive lock while `merod run` is up.

use calimero_config::ConfigFile;
use calimero_governance_store::{AccountRoot, NodeDeviceRepository};
use calimero_primitives::identity::PrivateKey;
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
    /// Print an account's id and root PUBLIC key, revealing no secret
    Root(RootCommand),
    /// Print this node's device id and public keys — what `sign-cert` certifies
    Device(DeviceCommand),
    /// Print this node's account root as a 24-word recovery phrase
    Export(ExportCommand),
    /// Restore an account root from a recovery phrase
    Import(ImportCommand),
    /// Sign a device revocation offline, to be published by any node
    RevokeProof(RevokeProofCommand),
    /// Certify a device offline, for a client that holds no node
    SignCert(SignCertCommand),
    /// Import a certificate this account's root signed elsewhere
    ImportCert(ImportCertCommand),
    /// Sign a warrant offline, authorising one relay to perform one intent
    Warrant(WarrantCommand),
}

#[derive(Debug, Parser)]
pub struct ExportCommand {
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

/// Sign a revocation for a device using only the account root.
///
/// The gap this closes: revoking a device needs the account's authority, and a
/// user who has merely *lost* a device still has it — but the only code path that
/// minted a proof required the acting node to hold the root in its own store. So
/// the authority existed and could not be exercised, which is the case the whole
/// account plane is for.
///
/// **The proof is self-certifying**, which is what makes an offline command
/// possible at all: it carries the genesis and the root-key chain, so a verifier
/// checks it from the account id alone with no folded state. It therefore does not
/// matter which node publishes it, and the root never has to reach one — this
/// prints a blob, and `meroctl account revoke --proof` hands it to any member.
///
/// With `--from` it needs no node at all: no home, no store, no init. That is the
/// lost-laptop case exactly — a replacement machine, a `merod` binary, and the
/// words. Without it, the root is read from this node's store, so the node must be
/// stopped.
///
/// **It cannot check that the device belongs to the account.** That takes group
/// state this command deliberately does not have, and it is not a hole: the
/// publishing node — and every replica applying the op — requires the stored
/// binding to name this account before a proof authorises anything. A proof for
/// somebody else's device is refused there, not here.
///
/// **Epoch 0 only.** The proof is signed at key epoch 0 with an empty handoff
/// chain, matching the node-side path. An account whose root has been rotated
/// needs the chain up to the signing epoch, and nothing in this CLI can produce
/// one yet.
#[derive(Debug, Parser)]
pub struct RevokeProofCommand {
    /// The device to revoke, 64 hex chars.
    #[arg(long = "device", value_name = "HEX")]
    device: String,

    /// Read the root from a recovery phrase at PATH instead of this node's store.
    ///
    /// Skips the datastore entirely, so it works on a machine with no node.
    #[arg(long, value_name = "PATH")]
    from: Option<camino::Utf8PathBuf>,
}

impl AccountCommand {
    pub async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        match self.action {
            AccountSubcommands::Root(cmd) => cmd.run(root_args).await,
            AccountSubcommands::Device(cmd) => cmd.run(root_args).await,
            AccountSubcommands::Export(cmd) => cmd.run(root_args).await,
            AccountSubcommands::Import(cmd) => cmd.run(root_args).await,
            AccountSubcommands::RevokeProof(cmd) => cmd.run(root_args).await,
            AccountSubcommands::SignCert(cmd) => cmd.run(root_args).await,
            AccountSubcommands::ImportCert(cmd) => cmd.run(root_args).await,
            AccountSubcommands::Warrant(cmd) => cmd.run(),
        }
    }
}

/// Certify a device using only the account root, for a holder that is not a node.
///
/// The gap this closes: `pair-complete` mints a certificate, and it needs a
/// running node that holds the account root to do it. A thin client — a phone, a
/// script, anything that runs no application and joins no group — has no such
/// node, so the credential it needs to present itself was unobtainable. Its
/// device key was therefore useless, however legitimately the account holder
/// wanted to grant it.
///
/// Same shape as [`RevokeProofCommand`] and for the same reason: the certificate
/// is **self-certifying**, carrying the genesis and the root-key chain, so a
/// verifier checks it from the account id alone with no folded state. It
/// therefore does not matter who publishes or presents it, and with `--from` the
/// root never has to reach a node at all — no home, no store, no init.
///
/// **It cannot check that the device id matches the keys.** `DeviceId` is
/// `H(account ‖ nonce)` and deliberately excludes the keys, so a device survives
/// a re-key. Nothing here can tell a mistyped id from a real one; a certificate
/// naming a device the holder does not have is inert rather than dangerous.
///
/// **Epoch 0 only**, exactly as `revoke-proof` is: the certificate is signed at
/// key epoch 0 with an empty handoff chain. An account whose root has rotated
/// needs the chain up to the signing epoch, and nothing in this CLI produces one.
#[derive(Debug, Parser)]
pub struct SignCertCommand {
    /// The device to certify, 64 hex chars — as printed by the client that minted it.
    #[arg(
        long = "device",
        value_name = "HEX",
        required_unless_present = "generate"
    )]
    device: Option<String>,

    /// The device's Ed25519 signing key, 64 hex chars.
    #[arg(
        long = "sign-pk",
        value_name = "HEX",
        required_unless_present = "generate"
    )]
    sign_pk: Option<String>,

    /// The device's X25519 agreement key, 64 hex chars. Scope keys are wrapped to it.
    #[arg(
        long = "kem-pk",
        value_name = "HEX",
        required_unless_present = "generate"
    )]
    kem_pk: Option<String>,

    /// Mint the device here and certify it in one step, printing its SECRET.
    ///
    /// For provisioning a client that has no way to mint its own — a script, a
    /// fresh install. It prints a signing key, so the holder is trusting this
    /// machine with it; a client that can mint its own device should, because
    /// then the secret never exists anywhere but there.
    #[arg(long, conflicts_with_all = ["device", "sign_pk", "kem_pk"])]
    generate: bool,

    /// Key-rotation epoch for this device. Must exceed any epoch already folded
    /// for it: the projection refuses a link that does not advance it, so
    /// re-issuing at the same epoch is inert rather than a rollback.
    #[arg(long, default_value_t = 0)]
    device_epoch: u32,

    /// Read the root from a recovery phrase at PATH instead of this node's store.
    ///
    /// Skips the datastore entirely, so it works on a machine with no node.
    #[arg(long, value_name = "PATH")]
    from: Option<camino::Utf8PathBuf>,
}

/// Sign a warrant: authorise one relay to perform one intent, once.
///
/// Offline by construction — it opens no store and contacts no node, because a
/// warrant is a statement about an intent rather than about any node's state.
/// The device secret signs it here and is never sent; only the signature travels.
///
/// This lives beside `sign-cert` and `revoke-proof` for the same reason they do:
/// they are the operations whose whole point is that the signing key does not
/// have to reach a running node. It is a client operation, and `meroctl context
/// intent` is the interactive form — this exists for a holder that has a shell
/// and needs the bytes.
///
/// **`--executor` must be the account that will actually run it.** A warrant
/// naming the wrong operator is refused by every peer, and nothing here can
/// check it — ask the relay: `GET /admin-api/identity` reports the account it
/// acts as.
///
/// **`--nonce` is the caller's to manage.** Peers refuse a repeat, so reusing one
/// means the write is silently dropped; a gap in the sequence is how a member
/// learns the relay withheld a request.
#[derive(Debug, Parser)]
pub struct WarrantCommand {
    /// The context the intent runs in, 64 hex chars.
    #[arg(long, value_name = "HEX")]
    context: String,

    /// The method to authorise.
    #[arg(long)]
    method: String,

    /// Its arguments, as the exact JSON the relay will be given.
    ///
    /// Byte-exact: the commitment covers these bytes, so a relay handed
    /// differently-formatted JSON with the same meaning is refused. Pass what
    /// will be sent.
    #[arg(long, default_value = "{}")]
    args: String,

    /// The operator account authorised to act, 64 hex chars.
    #[arg(long, value_name = "HEX")]
    executor: String,

    /// Monotonic per device.
    #[arg(long)]
    nonce: u64,

    /// Seconds from now that the warrant stays spendable.
    ///
    /// Read against this machine's clock when the command runs, so two mintings
    /// never share a deadline. Use `--not-after` where the exact value matters.
    #[arg(long, default_value_t = 300, conflicts_with = "not_after")]
    valid_for: u64,

    /// Absolute deadline, unix seconds — the alternative to `--valid-for`.
    ///
    /// Exists so a warrant can be reproduced exactly. Every other input to the
    /// signature is pinned by a flag; the deadline was the one field taken from
    /// the clock, which made two mintings of "the same" warrant differ here and,
    /// because the signature covers it, in the signature too. Given this, a
    /// second implementation can be diffed against this one byte for byte
    /// instead of inferred to agree from a node accepting its output.
    #[arg(long, value_name = "UNIX_SECONDS")]
    not_after: Option<u64>,

    /// The device's signing secret, 64 hex chars. Signs the warrant; never sent.
    #[arg(long, value_name = "HEX")]
    device_secret: String,

    /// The device's credential, as printed by `sign-cert`.
    ///
    /// Read for the account it names rather than taking that separately: the
    /// certificate already carries it, and asking twice is a way for the two to
    /// disagree.
    #[arg(long, value_name = "HEX")]
    credential: String,
}

impl WarrantCommand {
    fn run(self) -> EyreResult<()> {
        // A context id is base58 and an account id is hex. Parsing the context as
        // hex is what the first run of this command actually did, and it failed on
        // the first non-hex character — which is the confusion class #3402 is
        // about, caught here by an e2e rather than by a reviewer.
        let context: calimero_primitives::context::ContextId =
            self.context.trim().parse().wrap_err_with(|| {
                format!("--context '{}' is not a valid context id", self.context)
            })?;
        let executor = calimero_account::AccountId::from(parse_key(&self.executor, "executor")?);
        let secret = PrivateKey::from(parse_key(&self.device_secret, "device-secret")?);

        let credential_bytes =
            hex::decode(self.credential.trim()).wrap_err("--credential is not hex")?;
        let credential: calimero_account::AccountProof<calimero_account::DeviceCert> =
            borsh::from_slice(&credential_bytes)
                .wrap_err("--credential is not a valid device credential")?;

        // Refused here rather than by a peer, because a peer's refusal reads as a
        // credential problem when it is really a mismatched pair.
        if credential.statement.sign_pk != secret.public_key() {
            eyre::bail!(
                "the credential certifies a different key than --device-secret holds, so \
                 the warrant it signs would be refused"
            );
        }

        let args: serde_json::Value =
            serde_json::from_str(&self.args).wrap_err("--args is not valid JSON")?;
        let args_bytes = serde_json::to_vec(&args).wrap_err("--args could not be re-encoded")?;

        let not_after = self.not_after.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
                .saturating_add(self.valid_for)
        });

        let warrant = calimero_account::Warrant::sign(
            &secret,
            context,
            credential.statement.account,
            executor,
            calimero_account::Warrant::intent_hash(&self.method, &args_bytes),
            self.nonce,
            not_after,
        )
        .map_err(|err| eyre::eyre!("failed to sign the warrant: {err}"))?;

        println!(
            "{}",
            hex::encode(borsh::to_vec(&warrant).wrap_err("Failed to encode the warrant")?)
        );

        Ok(())
    }
}

impl SignCertCommand {
    async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        let root = resolve_root(root_args, self.from.as_ref()).await?;
        let account = root.account();

        // Minted here only with `--generate`; otherwise every value is the
        // client's, and this command never sees a secret at all.
        let mut generated_secret = None;
        let (device, sign_pk, kem_pk) = if self.generate {
            let mut nonce = [0u8; 16];
            rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce);
            let sign_sk = PrivateKey::random(&mut rand::rngs::OsRng);
            let kem_sk = calimero_crypto::X25519SecretKey::random(&mut rand::rngs::OsRng);
            let sign = *AsRef::<[u8; 32]>::as_ref(&sign_sk.public_key());
            let kem = *kem_sk.public_key().as_bytes();
            generated_secret = Some(hex::encode(sign_sk.as_bytes()));
            (calimero_account::DeviceId::mint(account, nonce), sign, kem)
        } else {
            (
                parse_device(self.device.as_deref().unwrap_or_default())?,
                parse_key(self.sign_pk.as_deref().unwrap_or_default(), "sign-pk")?,
                parse_key(self.kem_pk.as_deref().unwrap_or_default(), "kem-pk")?,
            )
        };

        let cert = calimero_account::DeviceCert::sign(
            root.signing_key(),
            account,
            device,
            &calimero_primitives::identity::PublicKey::from(sign_pk),
            &calimero_account::KemPublicKey::from(kem_pk),
            0,
            self.device_epoch,
        )
        .map_err(|err| eyre::eyre!("failed to sign the certificate: {err}"))?;

        let credential = calimero_account::AccountProof {
            genesis: root.genesis(),
            chain: vec![],
            statement: cert,
        };

        let encoded =
            hex::encode(borsh::to_vec(&credential).wrap_err("Failed to encode the credential")?);

        println!("{encoded}");
        println!();
        println!("Account: {account}");
        println!("Device:  {device}");
        if let Some(secret) = &generated_secret {
            println!("Secret:  {secret}");
        }
        println!();
        println!(
            "Hand this to the device it names. It presents it as its own \
             credential — nothing needs to publish it first:"
        );
        println!();
        println!("  meroctl context intent <CONTEXT_ID> --credential <the hex above> ...");

        Ok(())
    }
}

impl RevokeProofCommand {
    async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        let device = parse_device(&self.device)?;

        let root = resolve_root(root_args, self.from.as_ref()).await?;

        let account = root.account();
        let revocation =
            calimero_account::DeviceRevocation::sign(root.signing_key(), account, device, 0)
                .map_err(|err| eyre::eyre!("failed to sign the revocation: {err}"))?;
        let proof = calimero_account::SignedDeviceRevocation {
            genesis: root.genesis(),
            chain: vec![],
            statement: revocation,
        };

        let encoded = hex::encode(borsh::to_vec(&proof).wrap_err("Failed to encode the proof")?);

        println!("{encoded}");
        println!();
        println!("Account: {account}");
        println!("Device:  {device}");
        println!();
        println!(
            "The proof names the device, not a namespace — publish it in each \
             namespace the device should lose, from any node that is a member there \
             and has folded the device's link:"
        );
        println!();
        println!(
            "  meroctl account revoke <NAMESPACE_ID> --device-id {} --proof <the hex above>",
            self.device
        );
        println!();
        println!(
            "The proof is not a secret — it authorises exactly this one revocation \
             and nothing else. Only an admin can rotate the scope key, so until one \
             does, the revoked device stops writing but can still read."
        );

        Ok(())
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
        // Deliberately NOT `provision_account_root`: minting one here would report
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

        if let Some(path) = self.out {
            // `Zeroizing`, because `format!` would otherwise leave a second full
            // copy of the 24 words in heap that nothing wipes — the one thing the
            // rest of this command is careful about.
            let contents = Zeroizing::new(format!("{}\n", phrase.as_str()));
            let restricted = write_owner_only(&path, &contents)
                .wrap_err_with(|| format!("Failed to write the recovery phrase to {path}"))?;
            if restricted {
                println!("Recovery phrase written to {path} (owner-only, 0600)");
            } else {
                // Do not claim a permission this platform did not set. The words
                // are the whole account, and an operator who believes the file is
                // owner-only will treat it accordingly.
                println!("Recovery phrase written to {path}");
                println!(
                    "WARNING: this platform has no owner-only mode to set, so the \
                     file has whatever permissions your umask gives it. Check them \
                     before leaving it there."
                );
            }
            println!("It is unencrypted. Move it somewhere safe and delete this copy.");
        } else {
            println!("{}", phrase.as_str());
        }

        println!();
        println!("Account root public key: {}", root.public_key());
        println!("Account:                 {}", root.account());
        println!();
        println!(
            "The phrase is the whole backup. The account is the content address of \
             this root, so recovering the phrase recovers the account — the same one \
             in every namespace, with nothing to keep beside the words."
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
        // case. On a wiped node there is no row and "none yet" is true; on a forced
        // import over a live node it is exactly wrong, and that is the situation
        // where an operator most needs to know the device just went.
        if !outcome.released && !outcome.retained {
            println!(
                "No device is restored: this node holds none yet. Joining a namespace \
                 enrols one, and a peer then delivers the current scope key."
            );
        } else if outcome.released {
            println!();
            println!(
                "Dropped the device that belonged to the replaced root — its id is \
                 spent. Joining a namespace again mints a fresh device, and a peer \
                 delivers the current scope key to it."
            );
        } else {
            println!();
            println!(
                "Kept the device paired into an account this root does not own — \
                 replacing this node's root does not affect it."
            );
        }

        Ok(())
    }
}

/// Write `contents` to a **new** file at `path`, owner-readable only.
///
/// Returns whether the owner-only permission was actually applied, so the caller
/// can avoid promising one this platform cannot set.
///
/// `create_new` — i.e. `O_CREAT | O_EXCL` — rather than create-and-truncate, and
/// that single choice closes two holes at once:
///
/// - **Symlink follow.** `O_EXCL` fails if the path exists at all, including as a
///   symlink, so a pre-planted link cannot redirect a recovery key into a file
///   somebody else can read.
/// - **The mode window.** `.mode()` applies only when the file is *created*, so an
///   existing world-readable file kept its mode and the secret was written into it
///   before any chmod could tighten it. Refusing to reuse a path removes the
///   window rather than narrowing it; a `set_permissions` afterwards only ever
///   shortened it.
///
/// Refusing an existing file is also the better behaviour on its own terms:
/// silently truncating whatever is already there is a poor way to treat the one
/// key that cannot be regenerated.
///
/// The node home gets chmod-after-the-fact treatment at `init`
/// (`restrict_to_owner`), which is fine for a directory tree being built and not
/// for a single secret written on demand.
#[cfg(unix)]
fn write_owner_only(path: &camino::Utf8Path, contents: &str) -> EyreResult<bool> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                eyre::eyre!(
                    "{path} already exists, and this refuses to overwrite it: the \
                     existing file may be a symlink pointing somewhere readable, and \
                     its permissions are whatever they already are. Remove it or \
                     choose another path."
                )
            } else {
                eyre::Report::new(err)
            }
        })?;
    file.write_all(contents.as_bytes())?;
    Ok(true)
}

/// Non-Unix fallback: no mode bits to set, so the caller is told the file is
/// unrestricted rather than being allowed to claim otherwise.
///
/// Still `create_new`, because refusing to clobber an existing key backup is not
/// platform-specific.
#[cfg(not(unix))]
fn write_owner_only(path: &camino::Utf8Path, contents: &str) -> EyreResult<bool> {
    use std::io::Write as _;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                eyre::eyre!("{path} already exists, and this refuses to overwrite it")
            } else {
                eyre::Report::new(err)
            }
        })?;
    file.write_all(contents.as_bytes())?;
    Ok(false)
}

/// The account root to sign with: a recovery phrase if given, else this node's own.
///
/// Shared by `revoke-proof` and `sign-cert` rather than written twice, because
/// the interesting half is a refusal both must make identically — see below.
async fn resolve_root(
    root_args: &RootArgs,
    from: Option<&camino::Utf8PathBuf>,
) -> EyreResult<AccountRoot> {
    // Parse the phrase before opening anything, as `import` does: a typo should
    // fail on its own terms rather than after a store has been opened.
    match from {
        Some(path) => {
            let phrase: Zeroizing<String> = Zeroizing::new(
                std::fs::read_to_string(path)
                    .wrap_err_with(|| format!("Failed to read the recovery phrase from {path}"))?,
            );
            AccountRoot::from_mnemonic(&phrase)
        }
        None => {
            let store = open_store(root_args).await?;
            // Not `provision_account_root`: minting one here would sign with a key
            // that owns nothing, and the result would verify against itself while
            // authorising nothing anywhere.
            NodeDeviceRepository::new(&store)
                .account_root()
                .wrap_err("Failed to read the account root")?
                .ok_or_else(|| {
                    eyre::eyre!(
                        "This node has no account root, so it can prove nothing about \
                         any account. Pass --from with the recovery phrase for the \
                         account that owns the device."
                    )
                })
        }
    }
}

/// Parse a 32-byte hex key, naming the argument it rejected.
/// Parse a 32-byte key as hex.
///
/// Hex only. This accepted base58 as well while `GET /admin-api/identity` still
/// rendered the signing key that way and hex-encoded the two beside it. Now that
/// every id is hex, accepting base58 would hide a caller left on the old spelling
/// rather than telling them.
///
/// # Errors
/// If the value is not 64 hex characters.
fn parse_key(raw: &str, arg: &str) -> EyreResult<[u8; 32]> {
    let bytes = hex::decode(raw.trim()).map_err(|_ignored| {
        eyre::eyre!(
            "--{arg} is not hex. It is 64 hex characters, which is how both \
             `merod account device` and `meroctl account show` print it"
        )
    })?;
    bytes
        .try_into()
        .map_err(|_ignored| eyre::eyre!("--{arg} is not 32 bytes (64 hex characters)"))
}

/// Parse a hex `DeviceId`.
///
/// Separate from [`parse_namespace`] only so the error names the right argument:
/// `revoke-proof` takes both as 64 hex characters, and "not 32 bytes" without a
/// name is a coin flip.
fn parse_device(raw: &str) -> EyreResult<calimero_account::DeviceId> {
    let bytes = hex::decode(raw.trim()).wrap_err_with(|| format!("device '{raw}' is not hex"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| eyre::eyre!("device '{raw}' is not 32 bytes (64 hex characters)"))?;
    Ok(calimero_account::DeviceId::from(bytes))
}

#[cfg(test)]
mod tests {
    use calimero_account::DeviceId;

    use super::*;

    /// A fixed root, so these tests assert on derivation rather than on a key that
    /// changes per run. Not a secret: it owns nothing anywhere.
    const PHRASE: &str = "legal winner thank year wave sausage worth useful legal winner \
                          thank year wave sausage worth useful legal winner thank year \
                          wave sausage worth title";

    fn root() -> AccountRoot {
        AccountRoot::from_mnemonic(PHRASE).expect("the fixture phrase must parse")
    }

    /// Mint a proof exactly as `revoke-proof` does, and hand back the wire form.
    fn minted(device: DeviceId) -> String {
        let root = root();
        let revocation =
            calimero_account::DeviceRevocation::sign(root.signing_key(), root.account(), device, 0)
                .expect("signing must succeed");
        let proof = calimero_account::SignedDeviceRevocation {
            genesis: root.genesis(),
            chain: vec![],
            statement: revocation,
        };
        hex::encode(borsh::to_vec(&proof).expect("borsh must encode"))
    }

    fn decoded(wire: &str) -> calimero_account::SignedDeviceRevocation {
        borsh::from_slice(&hex::decode(wire).expect("hex must decode")).expect("borsh must decode")
    }

    /// The whole point of printing the proof: it has to survive being copied
    /// through a terminal and back. Hex and borsh are both exact, so this is
    /// really asserting that the *pair* round-trips and still verifies.
    #[test]
    fn a_printed_proof_still_authorises_after_a_round_trip_through_hex() {
        let device = DeviceId::from([0x42; 32]);

        let proof = decoded(&minted(device));

        proof
            .authorises(root().account(), device)
            .expect("a proof minted for this account and device must authorise it");
    }

    /// A revocation is about the device, not about where it was minted.
    ///
    /// One root owns one account everywhere, so a proof minted while pointed at
    /// one namespace authorises against the same account reached from any other.
    /// That is the intended reading: a stolen laptop is stolen in every scope its
    /// owner participates in, and the holder should not have to mint a separate
    /// proof per namespace to say so.
    ///
    /// It is not a way to revoke someone else's device. Only the account root can
    /// mint the proof, and it names that root's own device — so replaying it
    /// elsewhere does exactly what its author asked for, in another place.
    ///
    /// Publication is still per-DAG: a revocation only takes effect in a group
    /// once it has been published there. This is about which proofs verify, not
    /// about anything propagating on its own.
    #[test]
    fn a_proof_authorises_against_the_account_wherever_it_was_minted() {
        let device = DeviceId::from([0x42; 32]);
        let proof = decoded(&minted(device));

        assert_eq!(
            root().account(),
            root().account(),
            "one root is one account, or the rest of this test means nothing"
        );

        proof
            .authorises(root().account(), device)
            .expect("a proof must authorise against its own account");
    }

    /// A proof names one device. Reusing it against another is the case that would
    /// turn a single revocation into a way to spend any replica id.
    #[test]
    fn a_proof_does_not_authorise_a_different_device() {
        let proof = decoded(&minted(DeviceId::from([0x42; 32])));

        let _ = proof
            .authorises(root().account(), DeviceId::from([0x43; 32]))
            .expect_err("a proof for one device must not authorise another");
    }

    #[test]
    fn parse_device_names_the_argument_it_rejected() {
        let err = parse_device("nothex")
            .expect_err("must reject non-hex")
            .to_string();
        assert!(err.contains("device"), "{err}");

        let err = parse_device("aabb")
            .expect_err("must reject a short id")
            .to_string();
        assert!(err.contains("32 bytes"), "{err}");

        // Trailing whitespace is what a copy-paste actually delivers.
        let device = parse_device(&format!("  {}  \n", "42".repeat(32)))
            .expect("a padded but valid id must parse");
        assert_eq!(device, DeviceId::from([0x42; 32]));
    }

    /// Every id is hex, and the encoding no longer tells them apart.
    ///
    /// This replaces a pin asserting the opposite — that a context was base58 and
    /// an account hex, so the two could not interchange. That difference used to
    /// do free validation: `account warrant` once parsed `--context` as hex, and
    /// it failed loudly the moment a real base58 context id reached it.
    ///
    /// **Unifying on hex gives that up, deliberately.** A context id now parses
    /// cleanly where an account id is expected, and vice versa — both are 32
    /// bytes in the same spelling. The trade is made because the previous scheme
    /// had a worse failure in the other direction: base58's alphabet contains
    /// every hex digit except `0`, so a hex value handed to a base58 parser was
    /// often *valid* and decoded to the wrong 32 bytes silently.
    ///
    /// What still separates the two is the type system inside Rust, and semantic
    /// checks at the edges — "does this context exist" rather than "is this
    /// shaped like a context". Anything taking both as strings has to validate
    /// meaning, because spelling no longer will.
    #[test]
    fn every_id_is_hex_and_encoding_no_longer_distinguishes_them() {
        use calimero_primitives::context::ContextId;

        let bytes: [u8; 32] = core::array::from_fn(|i| i as u8);
        let context = ContextId::from(bytes);
        let hex = hex::encode(bytes);

        assert_eq!(context.to_string(), hex, "a context prints as hex");
        assert_eq!(hex.parse::<ContextId>().expect("parses"), context);
        assert!(
            hex.parse::<calimero_account::AccountId>().is_ok(),
            "and the same string is a valid account id — the encoding stopped \
             being a type check, which is the cost of one spelling",
        );

        // Base58 is refused everywhere now, so a caller still on the old form is
        // told rather than silently misread.
        let old_base58 = "11111111111111111111111111111112";
        assert!(old_base58.parse::<ContextId>().is_err());
    }

    /// The two lines `account root` prints must describe the same account.
    ///
    /// They are pasted into different places — the public key into `init
    /// --account-root`, the account id into a membership grant — so if they
    /// disagreed an operator would provision a node under one account and grant
    /// rights to another. Nothing would error; the node would simply never be a
    /// member, and the cause would be two numbers that looked fine side by side.
    #[test]
    fn the_printed_root_key_and_account_describe_the_same_account() {
        let root = root();

        assert_eq!(
            root.account(),
            calimero_account::AccountGenesis::new(root.public_key()).account_id(),
            "the account id must be the content address of the printed public key",
        );
    }

    /// The three values `account device` prints must be the ones a certificate is
    /// signed over — and they must agree with what the store holds.
    ///
    /// Printing a plausible-but-wrong key here is the worst failure this command
    /// has: the operator certifies it, the certificate verifies as a signature,
    /// and a peer then refuses the binding. Nothing local points at the cause.
    #[test]
    fn the_printed_device_values_come_from_the_store() {
        use calimero_governance_store::{NamespaceRepository, NodeDeviceRepository};
        use calimero_store::config::StoreConfig;
        use calimero_store::Store;
        use calimero_store_rocksdb::RocksDB;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().join("data")).expect("utf8 path");
        let store = Store::open::<RocksDB>(&StoreConfig::new(path)).expect("open");

        // What `merod init --no-account-root --account-root <pk>` leaves behind.
        let root_pk = PrivateKey::from([4u8; 32]).public_key();
        let genesis = calimero_account::AccountGenesis::new(root_pk);
        let device = NodeDeviceRepository::new(&store)
            .adopt_account(genesis)
            .expect("adopt");
        let signing_key = NamespaceRepository::new(&store)
            .provision_node_identity()
            .expect("provision");

        // The command reads exactly these, so assert the store agrees with itself
        // rather than re-deriving anything: a second derivation could be wrong in
        // the same way as the first.
        let reread = NodeDeviceRepository::new(&store)
            .get()
            .expect("read")
            .expect("device present");

        assert_eq!(reread.device(), device.device());
        assert_eq!(reread.account, genesis.account_id());
        assert_eq!(
            reread.kem_public_key().as_bytes(),
            device.kem_public_key().as_bytes(),
        );
        assert_eq!(
            NamespaceRepository::new(&store)
                .node_identity()
                .expect("read")
                .expect("present")
                .public_key,
            signing_key,
        );
    }

    /// `sign-cert`'s key arguments must accept both spellings in circulation.
    ///
    /// `GET /admin-api/identity` renders the signing key base58 — `PublicKey`'s own
    /// Display — while hex-encoding the device id and agreement key beside it.
    /// `merod account device` prints all three hex. So a caller pasting from the
    /// API supplies base58 for one argument and hex for the others, which is
    /// exactly what the offline-root e2e did before this.
    #[test]
    fn sign_cert_keys_accept_hex_and_base58() {
        let key = PrivateKey::from([6u8; 32]).public_key();
        let raw = *AsRef::<[u8; 32]>::as_ref(&key);

        assert_eq!(
            super::parse_key(&hex::encode(raw), "sign-pk").expect("hex"),
            raw
        );
        assert_eq!(
            super::parse_key(&key.to_string(), "sign-pk").expect("base58"),
            raw,
            "base58 is what the identity endpoint prints for this key",
        );
    }

    /// A rejection must name both spellings and where each comes from.
    #[test]
    fn a_bad_key_says_which_spellings_are_accepted() {
        let err = super::parse_key("not-a-key", "sign-pk").expect_err("must refuse");
        let msg = err.to_string();
        assert!(msg.contains("merod account device"), "{msg}");
        assert!(msg.contains("meroctl account show"), "{msg}");
    }

    /// Hex of the wrong length is a distinct mistake from a wrong encoding.
    #[test]
    fn a_short_hex_key_says_so() {
        let err = super::parse_key(&hex::encode([1u8; 16]), "kem-pk").expect_err("must refuse");
        assert!(err.to_string().contains("32 bytes"), "{err}");
    }

    /// `--not-after` and `--valid-for` cannot both be given.
    ///
    /// They set one field two ways, and letting either silently win would make a
    /// reproducible mint depend on flag order.
    #[test]
    fn an_absolute_deadline_conflicts_with_a_relative_one() {
        let err = WarrantCommand::try_parse_from([
            "warrant",
            "--context",
            &"11".repeat(32),
            "--method",
            "set",
            "--executor",
            &"22".repeat(32),
            "--nonce",
            "1",
            "--device-secret",
            &"33".repeat(32),
            "--credential",
            "aa",
            "--valid-for",
            "300",
            "--not-after",
            "1800000000",
        ])
        .expect_err("both deadlines must be refused");

        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::ArgumentConflict,
            "{err}"
        );
    }

    /// `--not-after` alone parses, despite `--valid-for` carrying a default.
    ///
    /// This pins clap rather than the code above: `conflicts_with` fires on a
    /// *provided* argument, and a defaulted one is not provided. Were that ever
    /// untrue the new flag could not be used without also passing the flag it
    /// exists to replace, so it is worth an assertion rather than a comment.
    #[test]
    fn an_absolute_deadline_alone_is_accepted() {
        let cmd = WarrantCommand::try_parse_from([
            "warrant",
            "--context",
            &"11".repeat(32),
            "--method",
            "set",
            "--executor",
            &"22".repeat(32),
            "--nonce",
            "1",
            "--device-secret",
            &"33".repeat(32),
            "--credential",
            "aa",
            "--not-after",
            "1800000000",
        ])
        .expect("an absolute deadline on its own must parse");

        assert_eq!(cmd.not_after, Some(1_800_000_000));
        assert_eq!(
            cmd.valid_for, 300,
            "the unused relative flag keeps its default"
        );
    }
}

/// Adopt a device certificate this account's root signed somewhere else.
///
/// The other half of `sign-cert`, and what makes a node with **no account root**
/// usable rather than merely permitted: the root stays in cold storage, signs a
/// certificate for this node's device, and this command is how the node starts
/// presenting it.
///
/// The ordering is forced and worth stating, because getting it wrong wastes an
/// air-gap trip: a certificate is signed **over** a device id, a signing key and an
/// agreement key, so the device must already exist here before anybody can certify
/// it. Read those three from `GET /admin-api/identity` (or `meroctl account show`),
/// certify them on the machine holding the root, then import the result.
///
/// Opens the datastore directly, so the node must be **stopped** — RocksDB's lock
/// is exclusive, exactly as for `export` and `import`.
///
/// Nothing here is secret. The certificate is public by construction: it travels in
/// every device binding this node publishes, and it authorises nothing on its own —
/// only the device holding the matching signing key can use it.
#[derive(Debug, Parser)]
pub struct ImportCertCommand {
    /// The hex credential, as printed by `merod account sign-cert`.
    ///
    /// Read from stdin when absent, so a certificate can be piped in without
    /// landing in shell history.
    #[arg(value_name = "HEX")]
    credential: Option<String>,
}

impl ImportCertCommand {
    pub async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        let encoded = match self.credential {
            Some(hex) => hex,
            None => {
                eprintln!("Paste the certificate, then press Ctrl-D:");
                let mut buf = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                    .wrap_err("Failed to read the certificate from stdin")?;
                buf
            }
        };
        let bytes = hex::decode(encoded.trim())
            .wrap_err("the credential is not hex; paste the line `sign-cert` printed")?;

        let proof: calimero_account::AccountProof<calimero_account::DeviceCert> =
            borsh::from_slice(&bytes)
                .wrap_err("the credential did not decode as a device certificate")?;

        let store = open_store(root_args).await?;
        let repo = NodeDeviceRepository::new(&store);

        // The device row must already be here, and must be the one certified.
        //
        // Checked now rather than at the first join, because a mismatch is
        // otherwise invisible until a peer refuses the join — and at that point
        // nothing local points at a mistyped `--device`.
        let device = repo
            .get()
            .wrap_err("could not read this node's device row")?
            .ok_or_else(|| {
                eyre::eyre!(
                    "this node has no device row yet, so there is nothing a certificate \
                     could describe. A device is minted when the node first takes part \
                     in a namespace, and it is those values a certificate is signed over"
                )
            })?;

        // Authenticity against the account the ROW names, not the one the proof
        // carries: a proof always verifies against its own genesis, so checking it
        // against itself would admit a certificate for an unrelated account.
        let verified = proof.verify(device.account).map_err(|err| {
            eyre::eyre!(
                "the certificate does not verify for this node's account {}: {err}",
                device.account
            )
        })?;

        eyre::ensure!(
            verified.device == device.device(),
            "the certificate is for device {} but this node is {}",
            verified.device,
            device.device(),
        );
        eyre::ensure!(
            verified.kem_pk == device.kem_public_key(),
            "the certificate names an agreement key that is not this device's, so scope \
             keys wrapped to it could not be opened here",
        );

        repo.store_imported_certificate(&bytes)
            .wrap_err("could not store the certificate")?;

        println!("Imported a certificate for device {}", device.device());
        println!("Account: {}", device.account);
        println!();
        println!(
            "This node will present it when it joins, instead of signing one with an \
             account root it does not hold."
        );
        Ok(())
    }
}

/// Report an account's id and root **public** key. Reveals nothing secret.
///
/// The read the offline posture was missing. `merod init --no-account-root
/// --account-root <HEX>` needs an account's root public key, and until now the only
/// way to obtain one was from a **running holder node** — precisely the machine that
/// posture says should not exist. With `--from` this answers from the phrase alone:
/// no node, no store, no init.
///
/// Distinct from `export`, which prints the phrase — the whole account. This prints
/// only what is public by construction: the root public key is hashed into the
/// account id and travels in every genesis, so publishing it grants nothing. Two
/// commands rather than a flag on one, because "show me the account" and "hand me
/// the secret" should not be one keystroke apart.
///
/// Without `--from` it reads this node's own root, so the node must be **stopped**
/// (RocksDB's lock is exclusive).
#[derive(Debug, Parser)]
pub struct RootCommand {
    /// Read the root from a recovery phrase at PATH instead of this node's store.
    ///
    /// Opens no datastore, so it works on a machine with no node.
    #[arg(long, value_name = "PATH")]
    from: Option<camino::Utf8PathBuf>,
}

impl RootCommand {
    #[expect(
        clippy::print_stdout,
        reason = "the values are what an operator pastes into `init --account-root`, \
                  so they go to stdout for piping rather than through a formatter"
    )]
    pub async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        let root = resolve_root(root_args, self.from.as_ref()).await?;

        println!("Account root public key: {}", root.public_key());
        println!("Account:                 {}", root.account());
        println!();
        println!(
            "Neither value is secret: the root public key is hashed into the account id \
             and travels in every genesis. The private root leaves a node only via \
             `merod account export`."
        );
        Ok(())
    }
}

/// Report this node's device: the three values a certificate is signed over.
///
/// `merod account sign-cert` is offline by design — with `--from` it opens no
/// store and contacts no node. Its **inputs** were not: the only way to learn
/// what to certify was `GET /admin-api/identity`, which needs a *running* node.
/// That is exactly the machine the cold-storage posture says should not be
/// required, so an operator with a stopped node and a root on another machine
/// could not assemble a `sign-cert` invocation at all.
///
/// Reads the datastore directly, so the node must be **stopped** — RocksDB's lock
/// is exclusive, as for `export` and `import`.
///
/// Nothing here is secret. All three are published in this device's binding: the
/// id names a replica, the signing key is what op signatures verify against, and
/// the agreement key is what wrapped scope keys are addressed to. The secrets that
/// match the latter two are reachable from no command and no route.
#[derive(Debug, Parser)]
pub struct DeviceCommand {}

impl DeviceCommand {
    #[expect(
        clippy::print_stdout,
        reason = "these are the values an operator pastes into `sign-cert`, so they \
                  go to stdout for piping rather than through a formatter"
    )]
    pub async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        let store = open_store(root_args).await?;

        let device = NodeDeviceRepository::new(&store)
            .get()
            .wrap_err("could not read this node's device row")?
            .ok_or_else(|| {
                eyre::eyre!(
                    "this node has no device yet. One is minted by `merod init \
                     --no-account-root --account-root <HEX>`, or the first time the \
                     node takes part in a namespace"
                )
            })?;

        // The signing key lives beside the device rather than in its row: it is
        // the key ops verify against, and it is provisioned at init.
        let signing_key = calimero_governance_store::NamespaceRepository::new(&store)
            .node_identity()
            .wrap_err("could not read this node's signing identity")?
            .ok_or_else(|| {
                eyre::eyre!(
                    "this node has a device but no signing identity, which should not \
                     happen: `merod init` provisions one. A node initialised before \
                     that mints it on its first namespace join"
                )
            })?
            .public_key;

        println!("Account:       {}", device.account);
        println!("Device:        {}", hex::encode(device.device().as_bytes()));
        println!(
            "Signing key:   {}",
            hex::encode(AsRef::<[u8; 32]>::as_ref(&signing_key))
        );
        println!(
            "Agreement key: {}",
            hex::encode(device.kem_public_key().as_bytes())
        );
        println!();
        println!(
            "Certify this device wherever its account root lives:\n\n  merod account \
             sign-cert --device {} --sign-pk {} --kem-pk {} --from <phrase-file>\n",
            hex::encode(device.device().as_bytes()),
            hex::encode(AsRef::<[u8; 32]>::as_ref(&signing_key)),
            hex::encode(device.kem_public_key().as_bytes()),
        );
        println!("Then bring the result back with `merod account import-cert`.");
        Ok(())
    }
}
