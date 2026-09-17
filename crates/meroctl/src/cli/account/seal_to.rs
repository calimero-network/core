use calimero_server_primitives::admin::{SealToAccountApiRequest, MAX_SEALABLE_PLAINTEXT_BYTES};
use clap::Parser;
use eyre::{bail, Result};
use tokio::io::{stdin, AsyncReadExt};

use crate::cli::Environment;

/// Seal a payload so that only one account's **root key** can open it.
///
/// The root, never a device key. A device is precisely what is gone in the case
/// worth sealing for — device loss — so an envelope addressed to one is
/// unopenable exactly when it is needed, and looks correct until then. This
/// command never takes a key for that reason: the node resolves the account's
/// current root itself.
///
/// The output proves **confidentiality, not authorship**. The sender key is
/// ephemeral and unauthenticated, so anyone who knows an account's root public
/// key can produce an envelope for it. Whatever decides that a stored envelope
/// is legitimate has to live in the service that accepted the write.
#[derive(Clone, Debug, Parser)]
#[command(about = "Seal stdin to an account's root key")]
pub struct SealToCommand {
    #[clap(
        name = "GROUP_ID",
        help = "The hex-encoded group whose view of the account to use"
    )]
    pub group_id: String,

    #[clap(
        name = "ACCOUNT",
        help = "The hex-encoded account ID to seal to (its root key is resolved by the node)"
    )]
    pub account: String,
}

impl SealToCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        // Raw bytes, not a line or a UTF-8 string: what gets sealed is usually a
        // serialized structure, and reading it as text would corrupt any byte
        // that is not valid UTF-8.
        let mut plaintext = Vec::new();
        let _ = stdin().read_to_end(&mut plaintext).await?;

        if plaintext.is_empty() {
            // Sealing nothing yields a valid envelope carrying no information,
            // which downstream is indistinguishable from a real one. Refusing
            // here means a caller whose input pipe was empty finds out now
            // rather than at recovery time.
            bail!("nothing on stdin: refusing to seal an empty payload");
        }
        if plaintext.len() > MAX_SEALABLE_PLAINTEXT_BYTES {
            bail!(
                "payload is {} bytes; the node seals at most {MAX_SEALABLE_PLAINTEXT_BYTES}",
                plaintext.len(),
            );
        }

        let client = environment.client()?;
        let response = client
            .seal_to_account(
                &self.group_id,
                &self.account,
                SealToAccountApiRequest {
                    plaintext: hex::encode(&plaintext),
                },
            )
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
