use core::str::FromStr;

use calimero_primitives::alias::{Alias, Kind};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::{eyre, Result as EyreResult};

use crate::Node;

pub fn resolve_identifier(
    node: &Node,
    input: &str,
    kind: Kind,
    context_id: Option<ContextId>,
) -> EyreResult<Hash> {
    let direct_result = match kind {
        Kind::Context => ContextId::from_str(input)
            .map(|context_id| context_id.into())
            .map_err(|_| eyre!("ContextId parsing failed")),
        Kind::Identity => PublicKey::from_str(input)
            .map(|public_key| public_key.into())
            .map_err(|_| eyre!("PublicKey parsing failed")),
        Kind::Application => return Err(eyre!("Application kind not supported")),
    };

    if let Ok(hash) = direct_result {
        return Ok(hash);
    }

    let alias = Alias::from_str(input)?;
    let store = node.store.handle();

    let key = node
        .ctx_manager
        .create_key_from_alias(kind, alias, context_id)?;

    store
        .get(&key)?
        .ok_or_else(|| eyre!("No {:?} found for alias '{}'", kind, input))
}
