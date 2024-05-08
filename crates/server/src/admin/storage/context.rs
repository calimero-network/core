use calimero_primitives::identity::Context;
use calimero_store::Store;

use super::did::{get_or_create_did, update_did};

pub fn add_context(store: &Store, context: Context) -> eyre::Result<bool> {
    let mut did_document = get_or_create_did(store)?;

    if !did_document.contexts.iter().any(|k| k.id == context.id) {
        did_document.contexts.push(context);
        update_did(store, did_document)?;
    }
    Ok(true)
}
pub fn delete_context(store: &Store, context_id: &str) -> eyre::Result<bool> {
    let mut did_document = get_or_create_did(store)?;

    match did_document
        .contexts
        .iter()
        .position(|k| k.id == context_id)
    {
        Some(position) => {
            did_document.contexts.remove(position);
            update_did(store, did_document)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

pub fn get_context(store: &Store, context_id: &str) -> eyre::Result<Option<Context>> {
    let did = get_or_create_did(store)?;
    Ok(did.contexts.into_iter().find(|k| k.id == context_id))
}

pub fn get_contexts(store: &Store) -> eyre::Result<Vec<Context>> {
    let did = get_or_create_did(store)?;
    Ok(did.contexts)
}
