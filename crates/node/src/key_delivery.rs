use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
use tracing::{info, warn};

/// After applying a namespace governance op, check if it was a `MemberJoined`
/// and we hold the group key. If so, publish a `KeyDelivery` wrapping the
/// group key for the joiner via ECDH.
pub async fn maybe_publish_key_delivery(
    context_client: &calimero_context_client::client::ContextClient,
    node_client: &calimero_node_primitives::client::NodeClient,
    op: &SignedNamespaceOp,
) {
    let NamespaceOp::Root(RootOp::MemberJoined {
        member,
        ref signed_invitation,
    }) = op.op
    else {
        return;
    };

    let namespace_id = op.namespace_id;
    let group_id_bytes = signed_invitation.invitation.group_id;
    let group_id = calimero_context_config::types::ContextGroupId::from(group_id_bytes);
    let ns_id = calimero_context_config::types::ContextGroupId::from(namespace_id);

    let store = context_client.datastore_handle().into_inner();

    let Some((_pk, sk_bytes, _)) =
        calimero_context::group_store::get_namespace_identity(&store, &ns_id)
            .ok()
            .flatten()
    else {
        return;
    };

    let Some((_key_id, group_key)) =
        calimero_context::group_store::load_current_group_key(&store, &group_id)
            .ok()
            .flatten()
    else {
        return;
    };

    let sender_sk = calimero_primitives::identity::PrivateKey::from(sk_bytes);
    let envelope = match calimero_context::group_store::wrap_group_key_for_member(
        &sender_sk, &member, &group_key,
    ) {
        Ok(env) => env,
        Err(e) => {
            warn!(?e, "failed to wrap group key for joiner");
            return;
        }
    };

    let delivery_op = NamespaceOp::Root(RootOp::KeyDelivery {
        group_id: group_id.to_bytes(),
        envelope,
    });

    if let Err(e) = calimero_context::group_store::sign_and_publish_namespace_op(
        &store,
        node_client,
        namespace_id,
        &sender_sk,
        delivery_op,
    )
    .await
    {
        warn!(?e, "failed to publish KeyDelivery");
        return;
    }

    info!(
        group_id = %hex::encode(group_id.to_bytes()),
        %member,
        "published KeyDelivery for new joiner"
    );
}
