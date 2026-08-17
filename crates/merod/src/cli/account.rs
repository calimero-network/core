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
    /// Sign a device revocation offline, to be published by any node
    RevokeProof(RevokeProofCommand),
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
            AccountSubcommands::Export(cmd) => cmd.run(root_args).await,
            AccountSubcommands::Import(cmd) => cmd.run(root_args).await,
            AccountSubcommands::RevokeProof(cmd) => cmd.run(root_args).await,
        }
    }
}

impl RevokeProofCommand {
    async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        let device = parse_device(&self.device)?;

        // Parse the phrase before opening anything, as `import` does: a typo should
        // fail on its own terms rather than after a store has been opened.
        let root = match &self.from {
            Some(path) => {
                let phrase: Zeroizing<String> =
                    Zeroizing::new(std::fs::read_to_string(path).wrap_err_with(|| {
                        format!("Failed to read the recovery phrase from {path}")
                    })?);
                AccountRoot::from_mnemonic(&phrase)?
            }
            None => {
                let store = open_store(root_args).await?;
                // Not `ensure_account_root`: minting one here would sign a proof
                // with a key that owns nothing, and it would verify against itself
                // while authorising nothing anywhere.
                NodeDeviceRepository::new(&store)
                    .account_root()
                    .wrap_err("Failed to read the account root")?
                    .ok_or_else(|| {
                        eyre::eyre!(
                            "This node has no account root, so it can prove nothing \
                             about any account. Pass --from with the recovery phrase \
                             for the account that owns the device."
                        )
                    })?
            }
        };

        let account = root.account();
        let revocation =
            calimero_account::sign_device_revocation(root.signing_key(), account, device, 0)
                .map_err(|err| eyre::eyre!("failed to sign the revocation: {err}"))?;
        let proof = calimero_account::SignedDeviceRevocation {
            genesis: root.genesis(),
            chain: vec![],
            revocation,
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
            calimero_account::sign_device_revocation(root.signing_key(), root.account(), device, 0)
                .expect("signing must succeed");
        let proof = calimero_account::SignedDeviceRevocation {
            genesis: root.genesis(),
            chain: vec![],
            revocation,
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
}
