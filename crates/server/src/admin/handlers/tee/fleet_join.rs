use std::sync::Arc;
use std::time::Duration;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::AccountId;
use calimero_context_client::group::{
    GroupContextEntry, JoinContextRequest, ListGroupContextsRequest,
};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{GroupKeyring, MembershipRepository, NamespaceRepository};
use calimero_node_primitives::client::TeeAdmissionParams;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::FleetJoinRequest;
use calimero_store::Store;
use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId};
use reqwest::StatusCode;
use tracing::{error, info, warn};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

/// Overall bound for one fleet-join call's wait for admission. The sidecar
/// re-invokes across a larger window, so this only needs to cover a single
/// mesh-formation attempt comfortably.
const MAX_ADMISSION_WAIT: Duration = Duration::from_secs(30);

/// Interval between admission checks AND between re-announces — short enough
/// that a transient mesh window (mesh peers appear, then vanish) is hit by a
/// fresh publish, but not so tight it spams the topic. Also the budget of one
/// context listing.
const ADMISSION_POLL: Duration = Duration::from_secs(2);

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<FleetJoinRequest>,
) -> impl IntoResponse {
    let group_id_bytes = match hex::decode(&req.group_id) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        _ => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "group_id must be 64 hex chars (32 bytes)".to_owned(),
            }
            .into_response();
        }
    };

    let group_id = ContextGroupId::from(group_id_bytes);

    info!(
        group_id = %req.group_id,
        "Fleet join: resolving namespace identity and generating attestation"
    );

    // Use namespace identity (per-root-group keypair) instead of a throwaway identity
    let (ns_id, our_public_key, our_sk) =
        match NamespaceRepository::new(&state.store).participate_in(&group_id) {
            Ok(result) => result,
            Err(err) => {
                error!(error=?err, "Failed to resolve namespace identity");
                return ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "Failed to resolve namespace identity".to_owned(),
                }
                .into_response();
            }
        };

    info!(
        %our_public_key,
        namespace_id = %hex::encode(ns_id.to_bytes()),
        "Using namespace identity for fleet join"
    );

    // A member has nothing to be admitted to, so answer before attesting or
    // asking anyone. The case is not hypothetical: the relay a namespace was
    // founded through is its first TEE member from the founding on, and the
    // sidecar still calls fleet-join on it — handing it its own addresses as
    // admitters. Asking itself hung this handler, and the sync loop serving the
    // request, forever.
    match already_admitted(&state.store, &group_id, &our_public_key) {
        Ok(Some(our_account)) => {
            info!(
                group_id = %req.group_id,
                %our_public_key,
                "Fleet join: already a member of the namespace and holding its key; \
                 answering admitted without asking for admission"
            );
            let entries = list_contexts_once(&state, group_id).await;
            let (contexts_joined, auto_follow_enabled) = settle_membership(
                &state,
                &req.group_id,
                group_id,
                our_public_key,
                our_sk,
                &entries,
            )
            .await;
            return fleet_join_response(
                &req.group_id,
                ns_id,
                our_public_key,
                our_account,
                true,
                auto_follow_enabled,
                contexts_joined,
            );
        }
        Ok(None) => {}
        // A read that failed says nothing either way; the join below is what
        // ran before this check existed, and it still converges for a member.
        Err(err) => warn!(
            group_id = %req.group_id,
            error = ?err,
            "Fleet join: could not read this node's membership; joining as a newcomer"
        ),
    }

    // The node release this node runs, when merod was told it
    // (`MERO_TEE_VERSION`). A namespace that admits TEEs by signed release
    // checks the quote against that release's signed measurements; without it
    // only a namespace with measurement lists can admit this node.
    let release_version = state.tee_release_version.clone();

    let announcement = match super::announce::build(
        &state.store,
        &ns_id,
        our_public_key,
        release_version.as_deref(),
        #[cfg(feature = "mock-attestation")]
        state.mock_tee,
    ) {
        Ok(announcement) => announcement,
        Err(err) => {
            let status_code = match err {
                super::announce::AnnounceError::MockRejected => StatusCode::NOT_IMPLEMENTED,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            return ApiError {
                status_code,
                message: err.message().to_owned(),
            }
            .into_response();
        }
    };

    // Reported because the caller cannot derive it: membership is recorded
    // against the ACCOUNT a key speaks for, so a caller holding only
    // `public_key` has nothing it can match a member listing against, and no
    // endpoint maps one to the other from outside the node that owns it.
    let account_id = announcement.account.statement.account;

    // This node cannot admit itself, and a dial to its own peer id is refused
    // by libp2p without the stream open it was meant to serve ever being
    // answered. The sync manager skips such routes too; dropping them here
    // means a list holding only our own addresses asks nobody.
    let admitter_addrs = if req.admitter_addrs.is_empty() {
        Vec::new()
    } else {
        without_own_addrs(
            &req.admitter_addrs,
            &state.node_client.local_peer_id().await,
        )
    };

    // Kept for the direct request below, which carries the same attestation the
    // broadcast does. Only built when there is someone to ask.
    let direct_request = (!admitter_addrs.is_empty()).then(|| TeeAdmissionParams {
        namespace_id: ns_id.to_bytes(),
        admitter_addrs,
        public_key: our_public_key,
        quote_bytes: announcement.quote_bytes.clone(),
        nonce: announcement.nonce,
        account: announcement.account.clone(),
        release_version: release_version.clone(),
    });

    let payloads = announcement.payloads;

    if let Err(err) = state.node_client.subscribe_namespace(group_id_bytes).await {
        error!(error=?err, "Failed to subscribe to namespace topic");
        return ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to subscribe to namespace".to_owned(),
        }
        .into_response();
    }

    // Fire the first announce up front. A single publish at fleet-join time is
    // lost forever if it lands in an empty gossipsub mesh (no replay), which is
    // the common case for a NAT'd/relay owner whose mesh forms only
    // intermittently. The admission loop below therefore RE-announces every
    // poll cycle until admitted or the deadline, so a *later* mesh window still
    // receives a fresh copy.
    //
    // An empty mesh at t=0 is the EXPECTED cold-start outcome, not a failure:
    // we subscribed to the namespace topic only moments ago, so no mesh peers
    // have formed yet and `publish_on_namespace_now` surfaces
    // `PublishError::NoPeersSubscribedToTopic`. Treating that first publish's
    // empty-mesh error as fatal (the bug #2491 fixes) returned a spurious 500
    // before the retry loop — whose whole purpose is to re-publish once the
    // mesh forms — was ever reached. So classify it the same way the loop does:
    // empty mesh is non-fatal, fall through into the retry loop below; any
    // *other* publish error is a genuine transport failure and still bails
    // (a subscription with no chance of an announce is useless).
    if let Err(err) = publish_announcements(&state.node_client, group_id_bytes, &payloads).await {
        if calimero_network_primitives::client::is_no_peers_subscribed_error(&err) {
            info!(
                group_id = %req.group_id,
                "First announce hit an empty gossipsub mesh (no peers subscribed yet); \
                 deferring to the re-announce loop, which republishes once the mesh forms"
            );
        } else {
            warn!(error=?err, "Failed to broadcast, unsubscribing from namespace");
            let _ = state
                .node_client
                .unsubscribe_namespace(group_id_bytes)
                .await;
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to broadcast attestation".to_owned(),
            }
            .into_response();
        }
    }

    info!(
        group_id = %req.group_id,
        %our_public_key,
        "TeeAttestationAnnounce broadcast; re-announcing until admission then joining contexts"
    );

    // Ask the named admitters directly, the way an invitation's joiner does.
    //
    // The broadcast above only works if a peer allowed to vouch is in the
    // gossip mesh to hear it, and when the only one is an owner's NAT'd laptop
    // that mesh may never form — the miss is silent. A direct request dials the
    // peer, gets a verdict back, and names why when it is refused. The broadcast
    // stays as the fallback for peers that predate this request, and it keeps
    // being re-announced below either way.
    //
    // Admission here is not the end of the job: this node still holds no
    // governance state and no key, so pull right away rather than waiting a
    // poll cycle. The loop below then confirms membership and joins contexts
    // exactly as it does after a broadcast admission.
    if let Some(params) = direct_request {
        match state.node_client.request_tee_admission(params).await {
            Ok(admitter) => {
                info!(
                    group_id = %req.group_id,
                    %admitter,
                    "admitted by a directly-asked admitter; pulling namespace governance"
                );
                if let Err(err) = state.node_client.sync_namespace(group_id_bytes).await {
                    tracing::debug!(
                        group_id = %req.group_id,
                        error = ?err,
                        "governance pull after direct admission failed; the loop retries it"
                    );
                }
            }
            Err(err) => warn!(
                group_id = %req.group_id,
                error = %format!("{err:#}"),
                "no admitter admitted this node directly; relying on the broadcast"
            ),
        }
    }

    // Poll for group admission, then auto-join all contexts in the namespace.
    //
    // Re-announce strategy: this loop both (a) checks for admission and (b)
    // re-publishes the announce each cycle the node is not yet admitted. The
    // re-announce is request-scoped (bounded by `MAX_ADMISSION_WAIT`) rather
    // than a long-lived background task: the mdma sidecar already re-polls
    // should-join and re-invokes fleet-join, so each call covering one mesh
    // window is sufficient, and a request-scoped loop needs no extra actor /
    // lifecycle management. See the handler-level rationale comment.
    let mut contexts_joined = Vec::new();
    let mut admitted = false;
    let mut auto_follow_enabled = false;

    let deadline = tokio::time::Instant::now() + MAX_ADMISSION_WAIT;

    // `loop {}` (not `while now < deadline`) so the deadline is only checked
    // *after* an admission check, never right after a sleep — otherwise an
    // admission that completes during the final sleep would be lost to a false
    // "timed out" / `admitted:false`. The deadline break lives in the `Err`
    // arm below, immediately after the (failed) admission check.
    loop {
        // Bound each admission check so a stuck context-manager actor can't
        // extend the handler past MAX_ADMISSION_WAIT: a check that exceeds the
        // poll interval is mapped to a (retriable) error and handled by the
        // `Err` arm below, exactly like a not-yet-admitted result. A
        // slow-but-not-stuck actor whose check nears ADMISSION_POLL makes the
        // effective cycle up to ~2x ADMISSION_POLL; that's acceptable, and we
        // keep the budget at ADMISSION_POLL (rather than shrinking it) so a
        // normally-fast actor isn't spuriously timed out. The overall deadline
        // still bounds total wall-clock either way.
        let admission = tokio::time::timeout(
            ADMISSION_POLL,
            state
                .ctx_client
                .list_group_contexts(ListGroupContextsRequest {
                    group_id,
                    offset: 0,
                    limit: 100,
                }),
        )
        .await
        .unwrap_or_else(|_| {
            Err(eyre::eyre!(
                "list_group_contexts exceeded the admission poll budget"
            ))
        });
        match admission {
            Ok(entries) => {
                info!(
                    group_id = %req.group_id,
                    context_count = entries.len(),
                    "Admitted to group, joining contexts"
                );
                admitted = true;

                let (joined, following) = settle_membership(
                    &state,
                    &req.group_id,
                    group_id,
                    our_public_key,
                    our_sk,
                    &entries,
                )
                .await;
                contexts_joined = joined;
                auto_follow_enabled = following;

                break;
            }
            Err(err) => {
                tracing::debug!(error=?err, "Admission check not yet successful, retrying...");

                // Stop once past the deadline — but only here, AFTER the
                // admission check above, so an admission that landed during the
                // previous sleep is observed on this iteration instead of being
                // lost to a false "timed out".
                if tokio::time::Instant::now() >= deadline {
                    break;
                }

                // Cap the poll sleep to the remaining budget so the loop wakes
                // for its final admission check right at the deadline rather
                // than up to ADMISSION_POLL past it.
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                tokio::time::sleep(remaining.min(ADMISSION_POLL)).await;

                // Re-announce AFTER the poll sleep, and only if we're still
                // before the deadline. Doing it here (rather than before the
                // sleep) avoids both a duplicate publish fired back-to-back with
                // the up-front one at t=0 and a wasted publish right as we give
                // up. A single up-front publish is lost if the mesh was empty at
                // fleet-join (gossipsub does not replay), so re-publishing each
                // cycle delivers a fresh copy to a mesh window that opens later.
                // Best effort — a transport error here is logged, not fatal.
                if tokio::time::Instant::now() < deadline {
                    match publish_announcements(&state.node_client, group_id_bytes, &payloads).await
                    {
                        Ok(mesh_peers) => tracing::debug!(
                            group_id = %req.group_id,
                            mesh_peers,
                            "re-announced TeeAttestationAnnounce while awaiting admission"
                        ),
                        Err(reannounce_err) => warn!(
                            group_id = %req.group_id,
                            error = ?reannounce_err,
                            "re-announce publish failed; will retry next cycle"
                        ),
                    }

                    // Bootstrap pull: a bare announcer holds NO namespace
                    // governance state (it only `subscribe_namespace`'d to send
                    // the announce). Once the verifier admits it, the verifier
                    // publishes the membership op (encrypted with the namespace
                    // group key) plus a `KeyDelivery` wrapping that key for this
                    // node — but both ride the namespace governance DAG, which
                    // this node has not pulled yet. The beacon-driven anti-entropy
                    // path deliberately skips a node with no local DAG head (it
                    // would race the bootstrap and pull undecryptable skeletons),
                    // so nothing pulls the DAG for us automatically. Trigger the
                    // pull ourselves each cycle: it fetches the full namespace
                    // governance DAG from a mesh peer, applies the `KeyDelivery`
                    // (decryptable with our namespace identity SK alone), then
                    // retries the previously-undecryptable membership op now that
                    // the group key is present. After that the `list_group_contexts`
                    // self-confirm above resolves and we join + replicate contexts.
                    // Best-effort: a missing mesh peer is logged inside
                    // `sync_namespace` and retried next cycle. Guarded by the same
                    // `now < deadline` check as the re-announce because it is a
                    // network op that should not run past the deadline.
                    if let Err(sync_err) = state.node_client.sync_namespace(group_id_bytes).await {
                        tracing::debug!(
                            group_id = %req.group_id,
                            error = ?sync_err,
                            "namespace governance bootstrap pull failed; will retry next cycle"
                        );
                    }
                }
            }
        }
    }

    if !admitted {
        warn!(
            group_id = %req.group_id,
            "Timed out waiting for group admission"
        );
    }

    fleet_join_response(
        &req.group_id,
        ns_id,
        our_public_key,
        account_id,
        admitted,
        auto_follow_enabled,
        contexts_joined,
    )
}

/// The fleet-join answer. One builder, so the already-a-member answer and the
/// admitted-by-asking one cannot drift apart in shape.
fn fleet_join_response(
    group_id: &str,
    ns_id: ContextGroupId,
    our_public_key: PublicKey,
    account: AccountId,
    admitted: bool,
    auto_follow_enabled: bool,
    contexts_joined: Vec<String>,
) -> axum::response::Response {
    ApiResponse {
        payload: serde_json::json!({
            "status": if admitted { "joined" } else { "announced" },
            "group_id": group_id,
            "namespace_id": hex::encode(ns_id.to_bytes()),
            "public_key": our_public_key.to_string(),
            "account": hex::encode(account.as_bytes()),
            "admitted": admitted,
            "auto_follow_enabled": auto_follow_enabled,
            "contexts_joined": contexts_joined,
        }),
    }
    .into_response()
}

/// The account this node is a member of `group_id` as, when it already is one
/// and holds the key that covers the group — that is, when there is nothing for
/// fleet-join to be admitted to.
///
/// Read from the membership rows rather than from a role: the relay a namespace
/// was founded through holds a `RelayTee` row there from the founding on, an
/// admitted replica a `ReadOnlyTee` one, and either is as much a member as an
/// admin. The key is required too, because a row without it is a member that
/// cannot read its namespace, which admission (and the key delivery behind it)
/// is still owed to.
fn already_admitted(
    store: &Store,
    group_id: &ContextGroupId,
    our_public_key: &PublicKey,
) -> eyre::Result<Option<AccountId>> {
    let Some(account) =
        calimero_governance_store::member_account_in_namespace(store, group_id, our_public_key)?
    else {
        return Ok(None);
    };
    if !MembershipRepository::new(store).is_member(group_id, &account)? {
        return Ok(None);
    }
    let covering = calimero_governance_store::key_covering_group(store, group_id)?;
    if GroupKeyring::new(store, covering)
        .load_current_key()?
        .is_none()
    {
        return Ok(None);
    }
    Ok(Some(account))
}

/// `admitter_addrs` without the ones naming this node.
///
/// A node cannot admit itself, and a dial to its own peer id is refused by
/// libp2p without the stream open waiting on it ever being answered. An address
/// that does not parse is kept: the sync manager drops it with its own reason.
fn without_own_addrs(admitter_addrs: &[String], local_peer: &PeerId) -> Vec<String> {
    admitter_addrs
        .iter()
        .filter(|addr| {
            let names_us = addr.parse::<Multiaddr>().is_ok_and(|parsed| {
                matches!(parsed.iter().last(), Some(Protocol::P2p(peer)) if peer == *local_peer)
            });
            if names_us {
                info!(%addr, "Fleet join: dropping an admitter address that names this node");
            }
            !names_us
        })
        .cloned()
        .collect()
}

/// The group's contexts, read once within one admission poll.
///
/// For a node that is already a member: it has nothing to wait for, so a
/// listing that fails is logged and answered with no contexts joined rather than
/// retried. Auto-follow, enabled alongside, joins them as they are seen.
async fn list_contexts_once(
    state: &AdminState,
    group_id: ContextGroupId,
) -> Vec<GroupContextEntry> {
    let listing = tokio::time::timeout(
        ADMISSION_POLL,
        state
            .ctx_client
            .list_group_contexts(ListGroupContextsRequest {
                group_id,
                offset: 0,
                limit: 100,
            }),
    )
    .await;
    match listing {
        Ok(Ok(entries)) => entries,
        Ok(Err(err)) => {
            warn!(error = ?err, "Fleet join: could not list the group's contexts");
            Vec::new()
        }
        Err(_elapsed) => {
            warn!("Fleet join: listing the group's contexts exceeded the admission poll budget");
            Vec::new()
        }
    }
}

/// What a member does once fleet-join knows it is one: join the group's
/// contexts and turn on auto-follow for itself. Returns the contexts joined and
/// whether auto-follow is on.
async fn settle_membership(
    state: &AdminState,
    group_label: &str,
    group_id: ContextGroupId,
    our_public_key: PublicKey,
    our_sk: [u8; 32],
    entries: &[GroupContextEntry],
) -> (Vec<String>, bool) {
    let mut contexts_joined = Vec::new();
    for entry in entries {
        match state
            .ctx_client
            .join_context(JoinContextRequest {
                context_id: entry.context_id,
            })
            .await
        {
            Ok(resp) => {
                info!(
                    context_id = %hex::encode(*resp.context_id),
                    "Joined context via group membership"
                );
                contexts_joined.push(hex::encode(*resp.context_id));
            }
            Err(err) => {
                warn!(
                    context_id = %hex::encode(*entry.context_id),
                    error = ?err,
                    "Failed to join context (may already be joined)"
                );
            }
        }
    }

    // Self-enable auto-follow now that we're a confirmed member.
    // Signed with our own namespace identity — satisfies the
    // admin-or-self authorization rule for MemberSetAutoFollow
    // (see the auto-follow architecture doc). The verifier that admitted us cannot do
    // this on our behalf because they're usually not admin and
    // don't hold our signing key. From here on, any new context
    // in the group auto-joins via the core auto-follow handler;
    // no sidecar polling needed.
    let our_sk_typed = calimero_primitives::identity::PrivateKey::from(our_sk);
    // The op names the account this replica acts as; admission wrote
    // its binding, so it resolves by the time we get here.
    // A read that FAILED is not "bound to no account". The warning
    // below tells an operator to wait for a binding that may already
    // exist, and the retry guidance above is written for a state this
    // would not actually be in.
    let our_account = calimero_governance_store::member_account_in_namespace(
        &state.store,
        &group_id,
        &our_public_key,
    )
    .unwrap_or_else(|err| {
        warn!(
            ?err,
            %our_public_key,
            "fleet-join: could not read this replica's account binding; skipping \
             auto-follow self-enable. This is a store fault, not a missing \
             binding — retrying the join will not help until it is resolved."
        );
        None
    });
    let Some(our_account) = our_account else {
        warn!(
            %our_public_key,
            "fleet-join: this replica's key is bound to no account; skipping \
             auto-follow self-enable. Admission succeeded, but subsequent \
             contexts will not auto-join."
        );
        return (contexts_joined, false);
    };

    // Already on: publishing the op again would change nothing and cost a
    // governance op on every call the sidecar makes.
    let already_following = MembershipRepository::new(&state.store)
        .member_value(&group_id, &our_account)
        .ok()
        .flatten()
        .is_some_and(|member| member.auto_follow.contexts && member.auto_follow.subgroups);
    if already_following {
        return (contexts_joined, true);
    }

    let auto_follow_enabled = match calimero_governance_store::sign_apply_and_publish(
        &state.store,
        &state.node_client,
        state.ctx_client.ack_router(),
        &group_id,
        &our_sk_typed,
        calimero_context_client::local_governance::GroupOp::MemberSetAutoFollow {
            target: our_account,
            auto_follow_contexts: true,
            auto_follow_subgroups: true,
        },
    )
    .await
    {
        Ok(report) => {
            report.observe("fleet_join", "MemberSetAutoFollow");
            info!(group_id = %group_label, "fleet-join: auto-follow enabled for self");
            true
        }
        Err(err) => {
            warn!(
                group_id = %group_label,
                ?err,
                "fleet-join: failed to enable auto-follow — admission succeeded but \
                 subsequent contexts will NOT auto-join until the op is retried. \
                 Operators can re-trigger fleet-join or publish MemberSetAutoFollow."
            );
            false
        }
    };
    (contexts_joined, auto_follow_enabled)
}

/// Publish each announcement form on the namespace topic, stopping at the
/// first failure. Returns the mesh size the last publish saw.
async fn publish_announcements(
    node_client: &calimero_node_primitives::client::NodeClient,
    namespace_id: [u8; 32],
    payloads: &[Vec<u8>],
) -> eyre::Result<usize> {
    let mut mesh_peers = 0;
    for payload in payloads {
        mesh_peers = node_client
            .publish_on_namespace_now(namespace_id, payload.clone())
            .await?;
    }
    Ok(mesh_peers)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use actix::{Actor, Context as ActixContext, Handler};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use axum::{Extension, Router};
    use calimero_context_client::client::ContextClient;
    use calimero_context_client::messages::ContextMessage;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{GroupKeyring, MembershipRepository, NamespaceRepository};
    use calimero_node_primitives::client::{
        BlobManager, NodeClient, SyncClient, TeeAdmissionParams, TeeAdmissionReply,
    };
    use calimero_primitives::context::GroupMemberRole;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::AutoFollowFlags;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use libp2p::PeerId;
    use tokio::sync::{broadcast, mpsc};
    use tower::ServiceExt;

    use super::{already_admitted, handler, without_own_addrs};
    use crate::{AdminState, NodeReadiness};

    const NAMESPACE: [u8; 32] = [0xF1; 32];

    /// Answers a context listing with no contexts; drops everything else.
    struct NoContexts;

    impl Actor for NoContexts {
        type Context = ActixContext<Self>;
    }

    impl Handler<ContextMessage> for NoContexts {
        type Result = ();

        fn handle(&mut self, msg: ContextMessage, _ctx: &mut Self::Context) {
            if let ContextMessage::ListGroupContexts { outcome, .. } = msg {
                let _ignored = outcome.send(Ok(Vec::new()));
            }
        }
    }

    /// This node as the relay a namespace was founded through: a `RelayTee` row
    /// at the root, the namespace key, and auto-follow already on. Returns its
    /// signing key.
    fn seat_founding_relay(
        store: &Store,
        with_key: bool,
    ) -> calimero_primitives::identity::PublicKey {
        let ns = ContextGroupId::from(NAMESPACE);
        let (_, our_public_key, _) = NamespaceRepository::new(store)
            .participate_in(&ns)
            .expect("this node's namespace identity");
        let account = calimero_context::test_support::enrol(store, &ns, &our_public_key);
        let membership = MembershipRepository::new(store);
        membership
            .add_member(&ns, &account, GroupMemberRole::RelayTee)
            .expect("the founding relay's row");
        membership
            .set_auto_follow(
                &ns,
                &account,
                AutoFollowFlags {
                    contexts: true,
                    subgroups: true,
                },
            )
            .expect("auto-follow on");
        if with_key {
            let _key_id = GroupKeyring::new(store, ns)
                .store_key(&[0x4B; 32])
                .expect("the namespace key it minted");
        }
        our_public_key
    }

    /// An admin state whose node client hands direct admission requests to the
    /// returned receiver, so a test can see whether anyone was asked.
    async fn admin_state(
        store: &Store,
    ) -> (
        Arc<AdminState>,
        mpsc::Receiver<(TeeAdmissionParams, TeeAdmissionReply)>,
        tempfile::TempDir,
    ) {
        let blob_dir = tempfile::TempDir::new().expect("tempdir");
        let blob_store = calimero_blobstore::BlobManager::new(
            store.clone(),
            calimero_blobstore::FileSystem::new(&calimero_blobstore::config::BlobStoreConfig::new(
                blob_dir.path().to_path_buf().try_into().expect("utf8 path"),
            ))
            .await
            .expect("blob fs"),
        );
        let (ctx_sync_tx, _r0) = mpsc::channel(8);
        let (ns_sync_tx, _r1) = mpsc::channel(8);
        let (ns_join_tx, _r2) = mpsc::channel(8);
        let (open_subgroup_join_tx, _r3) = mpsc::channel(8);
        let (relay_sealed_join_tx, _r4) = mpsc::channel(8);
        let (tee_tx, tee_rx) = mpsc::channel(8);
        let sync_client = SyncClient::new(
            ctx_sync_tx,
            ns_sync_tx,
            ns_join_tx,
            open_subgroup_join_tx,
            relay_sealed_join_tx,
        )
        .with_tee_admission(tee_tx);
        let (event_sender, _events) = broadcast::channel(16);
        let node_client = NodeClient::new(
            store.clone(),
            BlobManager::new(blob_store),
            calimero_network_primitives::client::NetworkClient::new(LazyRecipient::new()),
            crate::test_support::stub_node_manager(vec![]),
            event_sender,
            sync_client,
            None,
        );

        let context_manager = LazyRecipient::<ContextMessage>::new();
        let handle = context_manager.clone();
        let _addr = NoContexts::create(move |ctx| {
            assert!(handle.init(ctx), "context manager recipient init");
            NoContexts
        });
        let ctx_client = ContextClient::new(store.clone(), node_client.clone(), context_manager);

        let state = Arc::new(AdminState::new(
            store.clone(),
            ctx_client,
            node_client,
            Arc::new(NodeReadiness::new()),
            [0; 32],
            #[cfg(feature = "mock-attestation")]
            false,
        ));
        (state, tee_rx, blob_dir)
    }

    /// The prod hang: a founding relay, already the namespace's first TEE, is
    /// handed its own addresses as admitters. It must answer admitted at once,
    /// without attesting, announcing or asking anyone — asking itself never
    /// returned.
    #[actix::test]
    async fn a_member_is_answered_admitted_without_asking_anyone() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let our_public_key = seat_founding_relay(&store, true);
        let (state, mut tee_rx, _blob_dir) = admin_state(&store).await;

        let app = Router::new()
            .route("/tee/fleet-join", post(handler))
            .layer(Extension(state));
        let own_addr = format!("/ip4/10.0.0.1/tcp/2528/p2p/{}", PeerId::random());
        let body = serde_json::json!({
            "groupId": hex::encode(NAMESPACE),
            "admitterAddrs": [own_addr],
        });
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            app.oneshot(
                Request::post("/tee/fleet-join")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("a request"),
            ),
        )
        .await
        .expect("fleet-join answers a member promptly")
        .expect("the route answers");

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read the response");
        let answer: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(answer["admitted"], true, "{answer}");
        assert_eq!(answer["status"], "joined", "{answer}");
        assert_eq!(answer["auto_follow_enabled"], true, "{answer}");
        assert_eq!(answer["public_key"], our_public_key.to_string(), "{answer}");
        assert!(
            tee_rx.try_recv().is_err(),
            "a member asked for admission it already has"
        );
    }

    /// A row without the key is not yet a member that can read its namespace;
    /// admission, and the key delivery behind it, is still owed to it.
    #[test]
    fn a_member_row_without_the_key_is_not_already_admitted() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from(NAMESPACE);

        let keyless = seat_founding_relay(&store, false);
        assert_eq!(
            already_admitted(&store, &ns, &keyless).expect("readable"),
            None
        );

        let _key_id = GroupKeyring::new(&store, ns)
            .store_key(&[0x4B; 32])
            .expect("the namespace key");
        assert!(already_admitted(&store, &ns, &keyless)
            .expect("readable")
            .is_some());
    }

    /// A stranger — no binding, no row — is not.
    #[test]
    fn a_stranger_is_not_already_admitted() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from(NAMESPACE);
        let (_, our_public_key, _) = NamespaceRepository::new(&store)
            .participate_in(&ns)
            .expect("this node's namespace identity");

        assert_eq!(
            already_admitted(&store, &ns, &our_public_key).expect("readable"),
            None
        );
    }

    /// Addresses naming this node are dropped, every other one kept in order.
    #[test]
    fn admitter_addresses_naming_this_node_are_dropped() {
        let us = PeerId::random();
        let them = PeerId::random();
        let addrs = vec![
            format!("/ip4/10.0.0.1/tcp/2528/p2p/{us}"),
            format!("/ip4/10.0.0.2/tcp/2528/p2p/{them}"),
            format!("/ip4/10.0.0.1/udp/2528/quic-v1/p2p/{us}"),
            "not a multiaddr".to_owned(),
        ];

        assert_eq!(
            without_own_addrs(&addrs, &us),
            vec![
                format!("/ip4/10.0.0.2/tcp/2528/p2p/{them}"),
                "not a multiaddr".to_owned(),
            ]
        );
    }
}
