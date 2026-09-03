//! Mint a device for an account, entirely on this machine.
//!
//! The client half of getting a credential, and the counterpart to `merod
//! account sign-cert`. Where `account pair-init` mints a device **on a node**,
//! this mints one here — for a holder that has no node at all, which is the case
//! delegated authorship exists to serve.
//!
//! It prints a secret. That is the point: the secret is what signs warrants, and
//! it must never reach the node that runs them, because a node that held it could
//! forge writes in this account's name. Keep it as you would any signing key.
//!
//! The account root is not needed and not accepted. A device id is
//! `H(account ‖ nonce)`, so minting one takes only the account — and the device
//! stays inert until the root certifies it, which is the separate, deliberate
//! step that `sign-cert` performs. So this command can be run by anyone, for any
//! account, and gains them nothing.

use calimero_account::{AccountId, DeviceId, KemPublicKey};
use calimero_crypto::X25519SecretKey;
use calimero_primitives::identity::PrivateKey;
use clap::Parser;
use eyre::{Result, WrapErr};
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use rand::Rng;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Mint a device for an account on this machine, holding no node")]
pub struct DeviceInitCommand {
    #[clap(
        name = "ACCOUNT_ID",
        help = "The account this device will speak for, 64 hex chars"
    )]
    pub account_id: String,
}

impl DeviceInitCommand {
    #[expect(
        clippy::print_stdout,
        reason = "the secret must reach the operator and nothing else; a structured \
                  report would put a signing key through the output formatter"
    )]
    pub fn run(self, _environment: &mut Environment) -> Result<()> {
        let account: AccountId = self
            .account_id
            .trim()
            .parse()
            .wrap_err_with(|| format!("'{}' is not a valid account id", self.account_id))?;

        // Random, and not derived from the keys: the id must survive a re-key, so
        // binding it to the keys would cost the device its replica slot every
        // time it rotated.
        let mut nonce = [0u8; 16];
        UnwrapErr(SysRng).fill_bytes(&mut nonce);
        let device = DeviceId::mint(account, nonce);

        let sign_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
        let kem_sk = X25519SecretKey::random(&mut UnwrapErr(SysRng));

        println!("Device:      {device}");
        println!(
            "Signing key: {}",
            hex::encode(AsRef::<[u8; 32]>::as_ref(&sign_sk.public_key()))
        );
        println!(
            "Agreement:   {}",
            hex::encode(KemPublicKey::from(*kem_sk.public_key().as_bytes()).as_bytes())
        );
        println!();
        println!("SECRET — keep this, and never send it to a node:");
        println!("  {}", hex::encode(sign_sk.as_bytes()));
        println!();
        println!(
            "This device is inert until the account root certifies it. On whichever \
             machine holds that root:"
        );
        println!();
        println!(
            "  merod account sign-cert --device {} --sign-pk {} --kem-pk {} --from <phrase-file>",
            device,
            hex::encode(AsRef::<[u8; 32]>::as_ref(&sign_sk.public_key())),
            hex::encode(KemPublicKey::from(*kem_sk.public_key().as_bytes()).as_bytes()),
        );
        println!();
        println!("Then pass its output to `meroctl context intent --credential`.");

        Ok(())
    }
}
