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
//! * `--device-secret` — the key that signs the warrant. It never leaves this
//!   machine and is never sent: only the signature is. This is the whole reason
//!   the node cannot forge a write in your name.
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

use calimero_account::Warrant;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::PerformIntentApiRequest;
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
        value_name = "HEX",
        help = "Your device's signing secret, 64 hex chars. Signs the warrant; never sent"
    )]
    pub device_secret: String,

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

        let secret_bytes: [u8; 32] = hex::decode(self.device_secret.trim())
            .wrap_err("--device-secret is not hex")?
            .try_into()
            .map_err(|_ignored| eyre::eyre!("--device-secret is not 32 bytes (64 hex chars)"))?;
        let device_sk = PrivateKey::from(secret_bytes);

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

        // The commitment, not the intent. The envelope this rides in is plaintext
        // to anything subscribed to the context's topic, so the method and its
        // arguments stay sealed and only their hash travels in the clear.
        let warrant = Warrant::sign(
            &device_sk,
            context_id,
            author_account,
            executor,
            Warrant::intent_hash(&self.method, &args_bytes),
            self.nonce,
            not_after,
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
