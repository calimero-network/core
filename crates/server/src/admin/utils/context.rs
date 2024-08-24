use calimero_context::ContextManager;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{KeyPair, PublicKey};
use eyre::{eyre, Error as EyreError};

use super::identity::{generate_context_id, generate_identity_keypair};

#[derive(Debug)]
#[non_exhaustive]
pub struct ContextCreateResult {
    pub context: Context,
    pub identity: KeyPair,
}

pub async fn create_context(
    ctx_manager: &ContextManager,
    application_id: ApplicationId,
    private_key: Option<&str>,
    context_id: Option<ContextId>,
    initialization_params: Vec<u8>,
) -> Result<ContextCreateResult, EyreError> {
    let context_id = context_id.map_or_else(generate_context_id, |context_id| context_id);
    let context = Context::new(context_id, application_id, Hash::default());

    let initial_identity = if let Some(private_key) = private_key {
        // Parse the private key
        let private_key = bs58::decode(private_key)
            .into_vec()
            .map_err(|_| eyre!("Invalid private key"))?;
        let private_key: [u8; 32] = private_key
            .try_into()
            .map_err(|_| eyre!("Private key must be 32 bytes"))?;

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
        .create_context(&context, initial_identity, initialization_params)
        .await?;

    let context_create_result = ContextCreateResult {
        context,
        identity: initial_identity,
    };

    Ok(context_create_result)
}

pub async fn join_context(
    ctx_manager: &ContextManager,
    context_id: ContextId,
    private_key: Option<&str>,
) -> Result<(), EyreError> {
    let initial_identity = if let Some(private_key) = private_key {
        // Parse the private key
        let private_key = bs58::decode(private_key)
            .into_vec()
            .map_err(|_| eyre!("Invalid private key"))?;
        let private_key: [u8; 32] = private_key
            .try_into()
            .map_err(|_| eyre!("Private key must be 32 bytes"))?;

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

    let _ = ctx_manager
        .join_context(&context_id, initial_identity)
        .await?;

    Ok(())
}
