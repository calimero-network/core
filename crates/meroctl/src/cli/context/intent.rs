//! Ask a node to run one method on your behalf, under a warrant you sign here.
//!
//! This is the client half of delegated authorship, and it exists because a
//! device that cannot run the application still has to be the author of its own
//! writes. The node runs the method; the warrant minted here is what makes the
//! result yours rather than the node's, and what lets every peer check you asked
//! for it.
//!
//! # What it needs, and why each part
//!
//! * `--device-secret-file` — the key that signs the warrant (`-` reads stdin).
//!   It never leaves this machine and is never sent: only the signature is. This
//!   is the whole reason the node cannot forge a write in your name.
//! * `--credential` — the certificate proving that key is a device of your
//!   account, printed by `account pair-complete` on whichever device holds the
//!   account root. A peer verifies it from your account id alone, which is what
//!   lets a device that never joined the group be an author.
//! * `--nonce` — monotonic per device. Peers refuse a repeat, so this is what
//!   stops the node running one authorization twice; a gap in the sequence is
//!   also how you find out it dropped a request.
//!
//! You do **not** supply the node's own key. The warrant authorizes an operator
//! account and the node attaches its own credential — so which of its processes
//! runs the intent is not your problem, and a re-key on its side does not
//! invalidate a warrant you already signed.

use std::io::Read;

use calimero_account::{Warrant, WarrantTerms};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::PerformIntentApiRequest;
use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser;
use eyre::{Result, WrapErr};

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Ask a node to run a method on your behalf, under a warrant you sign")]
pub struct IntentCommand {
    #[clap(name = "CONTEXT_ID", help = "The context to run in")]
    pub context_id: String,

    #[clap(long, help = "The method to run")]
    pub method: String,

    #[clap(
        long,
        default_value = "{}",
        help = "Arguments as JSON, e.g. '{\"text\":\"hello\"}'"
    )]
    pub args: String,

    #[clap(
        long,
        value_name = "PATH",
        help = "File holding your device's signing secret, 64 hex chars; '-' reads stdin. \
                Signs the warrant; never sent"
    )]
    pub device_secret_file: Option<Utf8PathBuf>,

    #[clap(
        long,
        value_name = "HEX",
        conflicts_with = "device_secret_file",
        required_unless_present = "device_secret_file",
        help = "Your device's signing secret inline. Prefer --device-secret-file: an argument \
                is visible to other local users in the process list"
    )]
    pub device_secret: Option<String>,

    #[clap(
        long,
        value_name = "HEX",
        help = "Your device credential, as printed by `account pair-complete`"
    )]
    pub credential: String,

    #[clap(
        long,
        help = "Monotonic per device. Peers refuse a repeat, so reuse means the write is dropped"
    )]
    pub nonce: u64,

    #[clap(
        long,
        default_value_t = 300,
        value_name = "SECONDS",
        help = "How long the warrant stays spendable. Checked by the node, never by peers"
    )]
    pub valid_for: u64,
}

impl IntentCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let context_id: ContextId = self
            .context_id
            .parse()
            .wrap_err_with(|| format!("context '{}' is not a valid id", self.context_id))?;

        let device_sk = PrivateKey::from(resolve_device_secret(
            self.device_secret.as_deref(),
            self.device_secret_file.as_deref(),
            &mut std::io::stdin(),
        )?);

        // The credential names the account, so it does not have to be given
        // twice — and taking it from the certificate rather than from a flag
        // removes the way to get them inconsistent.
        let credential_bytes =
            hex::decode(self.credential.trim()).wrap_err("--credential is not hex")?;
        let credential: calimero_account::AccountProof<calimero_account::DeviceCert> =
            borsh::from_slice(&credential_bytes)
                .wrap_err("--credential is not a valid device credential")?;
        let author_account = credential.statement.account;

        if credential.statement.sign_pk != device_sk.public_key() {
            eyre::bail!(
                "this credential certifies a different key than --device-secret holds; \
                 a peer would refuse the warrant it signs"
            );
        }

        let args = self
            .args
            .parse::<serde_json::Value>()
            .wrap_err("--args is not valid JSON")?;
        let args_bytes = serde_json::to_vec(&args).wrap_err("--args could not be re-encoded")?;

        // Which operator is being authorized is read from the node, not asserted
        // here: the warrant has to name the account that will actually run it,
        // and a client guessing that would mint warrants nothing can spend.
        //
        // Read from the relay descriptor rather than from `identity`, because the
        // descriptor answers the other half too — whether this node may author at
        // all — and needs no credential on the node to answer either. See below
        // for why asking first matters.
        let client = environment.client()?;
        let relay = client
            .get_intent_relay(&self.context_id)
            .await
            .wrap_err("could not ask the node whether it can relay intents for this context")?;

        // Refuse here rather than after signing. `CAN_AUTHOR_ON_BEHALF` is implied
        // by nothing — not membership, not admin, not the subgroup cascade — so
        // "no" is the default answer and the ordinary one. Minting anyway spends
        // `--nonce` on a write every peer will reject, and the author cannot reuse
        // that number: the next attempt has to pick a higher one, and the gap is
        // permanent.
        if !relay.data.can_author_on_behalf {
            eyre::bail!(
                "this node's account ({}) may not author on behalf of members of this context, \
                 so the warrant would be refused and --nonce {} spent for nothing. Ask an admin \
                 of group {} to grant it:\n\n    meroctl group members set-capabilities {} {} \
                 --can-author-on-behalf\n\nThe mask is replaced, not merged, so re-pass any \
                 capability that account already holds.",
                relay.data.executor_account,
                self.nonce,
                relay.data.group_id,
                relay.data.group_id,
                relay.data.executor_account,
            );
        }

        let executor: calimero_account::AccountId = relay
            .data
            .executor_account
            .parse()
            .wrap_err("the node reported an account this client cannot parse")?;

        let not_after = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .saturating_add(self.valid_for);

        // The build the warrant is signed against, read from the node rather
        // than asserted here: `app_version` pins the code, so a value this
        // client guessed would pin the wrong one. Read after the relay check so
        // a refusal that costs nothing comes first.
        let app_version = client
            .get_context(&context_id)
            .await
            .wrap_err("could not read the context to learn which application it runs")?
            .data
            .application_id;

        // The arguments are the commitment, not the intent. The envelope this
        // rides in is plaintext to anything subscribed to the context's topic,
        // so the arguments stay sealed and only their hash travels in the clear;
        // the method itself is carried openly, so a peer can select a per-method
        // write-set without reversing a hash against the app's ABI.
        let warrant = Warrant::sign(
            &device_sk,
            WarrantTerms {
                context: context_id,
                author_account,
                executor,
                app_version,
                method: self.method.clone(),
                intent_hash: Warrant::intent_hash(&self.method, &args_bytes),
                // Cited by a client that tracks the logs; meroctl tracks
                // neither, and an empty list is the honest statement of that
                // rather than a fabricated view. Nothing verifies these yet
                // (#3933 lands the field set ahead of its enforcement so a
                // client builds against the final bytes once).
                account_heads: vec![],
                governance_floor: vec![],
                nonce: self.nonce,
                not_after,
            },
        )
        .wrap_err("could not sign the warrant")?;

        let response = client
            .perform_intent(
                &self.context_id,
                PerformIntentApiRequest {
                    method: self.method,
                    args_json: args,
                    warrant: hex::encode(
                        borsh::to_vec(&warrant).wrap_err("could not encode the warrant")?,
                    ),
                    author_proof: hex::encode(credential_bytes),
                },
            )
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}

/// Take the device secret from `--device-secret-file` (`-` is `stdin`) or from the
/// inline flag, trimmed and checked to be 32 bytes of hex.
fn resolve_device_secret(
    inline: Option<&str>,
    file: Option<&Utf8Path>,
    stdin: &mut dyn Read,
) -> Result<[u8; 32]> {
    let (raw, arg) = match (file, inline) {
        (Some(path), _) => {
            let mut raw = String::new();
            if path.as_str() == "-" {
                let _ = stdin
                    .read_to_string(&mut raw)
                    .wrap_err("failed to read --device-secret-file from stdin")?;
            } else {
                raw = std::fs::read_to_string(path)
                    .wrap_err_with(|| format!("failed to read --device-secret-file '{path}'"))?;
            }
            (raw, "--device-secret-file")
        }
        (None, Some(raw)) => (raw.to_owned(), "--device-secret"),
        (None, None) => eyre::bail!("--device-secret-file is required"),
    };

    let raw = raw.trim();
    if raw.is_empty() {
        eyre::bail!("{arg} is empty; it must be the secret as 64 hex characters");
    }
    hex::decode(raw)
        .wrap_err_with(|| format!("{arg} is not hex"))?
        .try_into()
        .map_err(|_ignored| eyre::eyre!("{arg} is not 32 bytes (64 hex chars)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse<'a>(extra: impl IntoIterator<Item = &'a str>) -> Result<IntentCommand, clap::Error> {
        let mut argv: Vec<String> = ["intent", "ctx", "--method", "set", "--credential"]
            .into_iter()
            .map(String::from)
            .collect();
        argv.extend(["dd".repeat(8), "--nonce".to_owned(), "1".to_owned()]);
        argv.extend(extra.into_iter().map(String::from));
        IntentCommand::try_parse_from(argv)
    }

    #[test]
    fn the_secret_comes_from_a_file_or_the_inline_flag_but_not_both() {
        let from_file = parse(["--device-secret-file", "-"]).expect("a file is enough");
        assert!(from_file.device_secret.is_none());

        let inline = "11".repeat(32);
        let _ = parse(["--device-secret", &inline]).expect("the inline flag keeps working");

        let err = parse(["--device-secret-file", "-", "--device-secret", &inline])
            .expect_err("both sources must be refused");
        assert!(err.to_string().contains("device-secret"), "{err}");

        let err = parse([]).expect_err("one source is required");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn a_secret_file_is_trimmed_and_validated() {
        let hex = "ab".repeat(32);
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = Utf8PathBuf::from_path_buf(dir.path().join("secret")).expect("utf-8 path");

        std::fs::write(&path, format!("\n {hex} \n")).expect("write the fixture");
        let key = resolve_device_secret(None, Some(&path), &mut std::io::empty())
            .expect("padded hex in a file must parse");
        assert_eq!(key, [0xab; 32]);

        let stdin = format!("{hex}\n").into_bytes();
        let key = resolve_device_secret(None, Some(Utf8Path::new("-")), &mut &stdin[..])
            .expect("`-` must read the stream given");
        assert_eq!(key, [0xab; 32]);

        std::fs::write(&path, "  \n").expect("write the fixture");
        let err = resolve_device_secret(None, Some(&path), &mut std::io::empty())
            .expect_err("an empty file is not a secret")
            .to_string();
        assert!(err.contains("--device-secret-file is empty"), "{err}");

        std::fs::write(&path, "abcd").expect("write the fixture");
        let err = resolve_device_secret(None, Some(&path), &mut std::io::empty())
            .expect_err("a short secret is refused")
            .to_string();
        assert!(err.contains("not 32 bytes"), "{err}");

        std::fs::write(&path, "not-hex").expect("write the fixture");
        let err = resolve_device_secret(None, Some(&path), &mut std::io::empty())
            .expect_err("non-hex is refused")
            .to_string();
        assert!(
            err.contains("is not hex") && !err.contains("not-hex"),
            "{err}"
        );

        let missing = path.with_file_name("absent");
        let err = resolve_device_secret(None, Some(&missing), &mut std::io::empty())
            .expect_err("a missing file is reported")
            .to_string();
        assert!(err.contains("failed to read --device-secret-file"), "{err}");

        let inline = resolve_device_secret(Some(&hex), None, &mut std::io::empty())
            .expect("the inline form keeps working");
        assert_eq!(inline, [0xab; 32]);
    }
}
