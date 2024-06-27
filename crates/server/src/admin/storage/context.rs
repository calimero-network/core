use calimero_primitives::context::{Context, ContextId};
use calimero_store::Store;

use super::did::{get_or_create_did, update_did};

pub fn add_context(store: &mut Store, context: Context) -> eyre::Result<bool> {
    let mut did_document = get_or_create_did(store)?;

    if !did_document.contexts.contains(&context.id) {
        did_document.contexts.push(context.id);
        update_did(store, did_document)?;
    }
    Ok(true)
}
pub fn delete_context(store: &mut Store, context_id: &ContextId) -> eyre::Result<bool> {
    let mut did_document = get_or_create_did(store)?;

    match did_document.contexts.iter().position(|id| id == context_id) {
        Some(position) => {
            did_document.contexts.remove(position);
            update_did(store, did_document)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

pub fn get_context(store: &mut Store, context_id: &ContextId) -> eyre::Result<Option<Context>> {
    let handle = store.handle();

    let key = calimero_store::key::ContextMeta::new(*context_id);

    let Some(context_meta) = handle.get(&key)? else {
        return Ok(None);
    };

    let context = Context {
        id: *context_id,
        application_id: context_meta.application_id.into_string().into(),
    };

    Ok(Some(context))
}

pub fn get_contexts(store: &mut Store) -> eyre::Result<Vec<Context>> {
    let handle = store.handle();

    let start = calimero_store::key::ContextMeta::new([0; 32].into());

    let mut contexts = vec![];

    for (key, context_meta) in handle.iter(&start)?.entries() {
        let context = Context {
            id: key.context_id(),
            application_id: context_meta.application_id.into_string().into(),
        };

        contexts.push(context);
    }

    Ok(contexts)
}
