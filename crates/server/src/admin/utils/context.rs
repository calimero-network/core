use calimero_primitives::context::Context;
use calimero_primitives::identity::{KeyPair, PublicKey};

use super::identity::{generate_context_id, generate_identity_keypair};

pub struct ContextCreateResult {
    pub context: Context,
    pub identity: KeyPair,
}

pub async fn create_context(
    ctx_manager: &calimero_context::ContextManager,
    application_id: calimero_primitives::application::ApplicationId,
    private_key: Option<&str>,
) -> Result<ContextCreateResult, eyre::Error> {
    let context_id = generate_context_id();
    let context = calimero_primitives::context::Context {
        id: context_id,
        application_id,
        last_transaction_hash: calimero_primitives::hash::Hash::default(),
    };

    let initial_identity = if let Some(private_key) = private_key {
        // Parse the private key
        let private_key = bs58::decode(private_key)
            .into_vec()
            .map_err(|_| eyre::eyre!("Invalid private key"))?;
        let private_key: [u8; 32] = private_key
            .try_into()
            .map_err(|_| eyre::eyre!("Private key must be 32 bytes"))?;

        // Generate the public key from the private key
        let public_key = PublicKey::derive_from_private_key(&private_key);
        // Create a KeyPair from the provided public and private keys
        KeyPair {
            public_key,
            private_key: Some(private_key),
        }
    } else {
        generate_identity_keypair()
    };

    ctx_manager
        .add_context(context.clone(), initial_identity.clone())
        .await?;

    let context_create_result = ContextCreateResult {
        context,
        identity: initial_identity,
    };

    Ok(context_create_result)
}