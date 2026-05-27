use calimero_context::group_store::{GroupKeyring, NamespaceRepository};
use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
use tracing::{info, warn};

/// After applying a namespace governance op, check if it was a member-join
/// op (`MemberJoined` for the admin-invited path, or `MemberJoinedOpen` for
/// the inherited Open-subgroup path #2351) and we hold the group key. If
/// so, publish a `KeyDelivery` wrapping the group key for the joiner via
/// ECDH.
pub async fn maybe_publish_key_delivery(
    context_client: &calimero_context_client::client::ContextClient,
    node_client: &calimero_node_primitives::client::NodeClient,
    op: &SignedNamespaceOp,
) {
    let (member, group_id) = match op.op {
        NamespaceOp::Root(RootOp::MemberJoined {
            member,
            ref signed_invitation,
        }) => (member, signed_invitation.invitation.group_id),
        NamespaceOp::Root(RootOp::MemberJoinedOpen { member, group_id }) => (
            member,
            calimero_context_config::types::ContextGroupId::from(group_id),
        ),
        _ => return,
    };

    let namespace_id = op.namespace_id;
    let ns_id = calimero_context_config::types::ContextGroupId::from(namespace_id);

    let store = context_client.datastore_handle().into_inner();

    let Some((_pk, sk_bytes, _)) = NamespaceRepository::new(&store)
        .identity(&ns_id)
        .ok()
        .flatten()
    else {
        return;
    };

    let Some((_key_id, group_key)) = GroupKeyring::new(&store, group_id)
        .load_current_key()
        .ok()
        .flatten()
    else {
        return;
    };

    let sender_sk = calimero_primitives::identity::PrivateKey::from(sk_bytes);
    let envelope = match GroupKeyring::wrap_for_member(&sender_sk, &member, &group_key) {
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

    // Pass `required_signers = Some([member])` so only the joiner's ack
    // counts toward delivery confirmation. The publisher boundary will
    // reflect the joiner's ack (or its absence) in the returned
    // `DeliveryReport.acked_by`; Phase 9.2 will gate `join_group` on
    // observing that ack before returning to the API caller.
    let report = match calimero_context::group_store::sign_and_publish_namespace_op(
        &store,
        node_client,
        context_client.ack_router(),
        namespace_id,
        &sender_sk,
        delivery_op,
        Some(vec![member]),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(?e, "failed to publish KeyDelivery");
            return;
        }
    };

    info!(
        group_id = %hex::encode(group_id.to_bytes()),
        %member,
        acked = report.acked_by.len(),
        elapsed_ms = report.elapsed_ms,
        "published KeyDelivery for new joiner"
    );
}
