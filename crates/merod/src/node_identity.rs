//! Node identity storage in the datastore.
//!
//! The libp2p keypair is stored in the datastore instead of config.toml.
//! When TEE is configured, the datastore is encrypted, so the keypair is
//! encrypted at rest with the same mechanism as other datastore values.

use calimero_store::key::Generic;
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::slice::Slice;
use calimero_store::Store;
use eyre::{Context, Result};
use libp2p::identity::Keypair;

/// Scope for merod node-local data in the Generic column.
const NODE_SCOPE: [u8; 16] = *b"merod_node\0\0\0\0\0\0";
/// Fragment for the libp2p identity keypair.
const IDENTITY_FRAGMENT: [u8; 32] = *b"libp2p_identity\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";

fn identity_key() -> Generic {
    Generic::new(NODE_SCOPE, IDENTITY_FRAGMENT)
}

/// Load the node identity from the datastore.
///
/// Returns `None` if no identity is stored (e.g. before migration from config).
pub fn load_from_store(store: &Store) -> Result<Option<Keypair>> {
    let key = identity_key();
    let value = store.get(&key).context("Failed to read identity from datastore")?;

    let Some(slice) = value else {
        return Ok(None);
    };

    let bytes = slice.as_ref().to_vec();
    let keypair = Keypair::from_protobuf_encoding(&bytes)
        .context("Invalid keypair in datastore")?;

    Ok(Some(keypair))
}

/// Save the node identity to the datastore.
///
/// Overwrites any existing identity.
pub fn save_to_store(store: &mut Store, keypair: &Keypair) -> Result<()> {
    let key = identity_key();
    let bytes = keypair
        .to_protobuf_encoding()
        .context("Failed to encode keypair")?;
    let value = Slice::from(bytes.to_vec());

    store.put(&key, value).context("Failed to write identity to datastore")?;
    store.commit().context("Failed to commit identity to datastore")?;

    Ok(())
}
