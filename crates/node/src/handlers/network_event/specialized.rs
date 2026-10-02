use actix::{AsyncContext, WrapFuture};
use calimero_node_primitives::sync::BroadcastMessage;
use tracing::{debug, warn};

use crate::NodeManager;

/// Why a gossipsub topic was rejected as a `TeeAdmissionPrompt`
/// namespace-governance topic.
#[derive(Debug, PartialEq, Eq)]
enum NamespaceTopicError {
    /// Topic did not carry the `ns/` namespace-governance prefix. Fleet
    /// TEE nodes publish on `ns/<hex(namespace_id)>` via
    /// `NodeClient::publish_on_namespace`, so anything else is not an
    /// admission prompt.
    NotNamespaceTopic,
    /// Topic had the `ns/` prefix but the suffix was not a 32-byte hex id.
    MalformedHex,
}

/// Parse a `TeeAdmissionPrompt` gossipsub topic into its namespace id.
///
/// Fleet TEE nodes prompt on `ns/<hex(namespace_id)>` (the namespace
/// governance topic — see `NodeClient::publish_on_namespace`,
/// `governance_broadcast::ns_topic`, and the `ns/` handling in
/// `subscriptions.rs`). The namespace IS its root group, so the returned
/// 32-byte id is used directly as the admission group id.
fn parse_namespace_prompt_topic(topic_str: &str) -> Result<[u8; 32], NamespaceTopicError> {
    let hex = topic_str
        .strip_prefix("ns/")
        .ok_or(NamespaceTopicError::NotNamespaceTopic)?;
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(hex, &mut bytes).map_err(|_| NamespaceTopicError::MalformedHex)?;
    Ok(bytes)
}

pub(super) fn handle_specialized_broadcast(
    this: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    source: libp2p::PeerId,
    topic: &libp2p::gossipsub::TopicHash,
    message: &BroadcastMessage<'_>,
) -> bool {
    match message {
        BroadcastMessage::TeeAdmissionPrompt => {
            let topic_str = topic.as_str();
            // Fleet TEE nodes prompt on the namespace governance topic
            // `ns/<hex(namespace_id)>` (see `NodeClient::publish_on_namespace`
            // and the `ns/` convention in `subscriptions.rs` /
            // `governance_broadcast::ns_topic`). The namespace IS its root
            // group, so the parsed namespace id is the admission group id.
            let namespace_id_bytes = match parse_namespace_prompt_topic(topic_str) {
                Ok(bytes) => bytes,
                Err(NamespaceTopicError::MalformedHex) => {
                    warn!(
                        %source,
                        topic = %topic_str,
                        "Invalid namespace topic hex in TeeAdmissionPrompt"
                    );
                    return true;
                }
                Err(NamespaceTopicError::NotNamespaceTopic) => {
                    warn!(
                        %source,
                        topic = %topic_str,
                        "TeeAdmissionPrompt received on non-namespace topic"
                    );
                    return true;
                }
            };

            debug!(
                %source,
                namespace_id = %hex::encode(namespace_id_bytes),
                "Received TEE admission prompt on namespace topic"
            );

            // The prompt admits nobody: a member that may vouch offers its source a
            // challenge, and only a quote over it is verified.
            let sync = this.managers.sync.clone();
            let _ignored = ctx.spawn(
                async move {
                    sync.offer_tee_challenge(namespace_id_bytes, source).await;
                }
                .into_actor(this),
            );
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_namespace_prompt_topic, NamespaceTopicError};

    /// Regression test for the `ns/` vs `group/` topic mismatch (PR #2096):
    /// fleet TEE nodes prompt with `TeeAdmissionPrompt` on
    /// `ns/<hex(namespace_id)>`, but the dispatcher used to strip
    /// `group/`, so the prompt fell into the "non-namespace topic" arm and was
    /// dropped — `offer_tee_challenge` never ran, and fleet TEE nodes were
    /// never admitted to the namespace group. The dispatcher must resolve an `ns/` topic to its namespace id
    /// and route it into the admission path.
    #[test]
    fn ns_prompt_topic_resolves_to_namespace_id_for_admission() {
        let namespace_id = [0x42u8; 32];
        let topic = format!("ns/{}", hex::encode(namespace_id));

        let parsed = parse_namespace_prompt_topic(&topic)
            .expect("ns/<hex> prompt topic must route into the admission path, not be dropped");

        // The resolved id is what gets handed to
        // `offer_tee_challenge` → `admit_tee_node` as the
        // admission group id (the namespace is its own root group).
        assert_eq!(parsed, namespace_id);
    }

    /// The old (buggy) `group/<hex>` topic must NOT match this path anymore.
    /// `group/` is not how TEE prompts are published (publish uses
    /// `publish_on_namespace` → `ns/`), so a `group/` topic here is a
    /// non-namespace topic and is correctly rejected rather than admitted.
    #[test]
    fn legacy_group_topic_is_not_a_namespace_prompt_topic() {
        let topic = format!("group/{}", hex::encode([0x42u8; 32]));
        assert_eq!(
            parse_namespace_prompt_topic(&topic),
            Err(NamespaceTopicError::NotNamespaceTopic),
        );
    }

    /// A non-prefixed topic (e.g. a raw context id) is not a namespace
    /// prompt topic.
    #[test]
    fn unprefixed_topic_is_not_a_namespace_prompt_topic() {
        assert_eq!(
            parse_namespace_prompt_topic("some-context-id"),
            Err(NamespaceTopicError::NotNamespaceTopic),
        );
    }

    /// An `ns/` topic with a malformed (non-hex / wrong-length) suffix is
    /// reported distinctly so the dispatcher can warn precisely instead of
    /// silently treating it as the wrong kind of topic.
    #[test]
    fn ns_topic_with_malformed_hex_is_rejected_as_malformed() {
        assert_eq!(
            parse_namespace_prompt_topic("ns/not-hex"),
            Err(NamespaceTopicError::MalformedHex),
        );
        // Right prefix, valid hex, wrong length (16 bytes, not 32).
        assert_eq!(
            parse_namespace_prompt_topic(&format!("ns/{}", hex::encode([0u8; 16]))),
            Err(NamespaceTopicError::MalformedHex),
        );
    }
}
