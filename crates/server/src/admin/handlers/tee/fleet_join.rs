use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::{JoinContextRequest, ListGroupContextsRequest};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::NamespaceRepository;
use calimero_server_primitives::admin::FleetJoinRequest;
use reqwest::StatusCode;
use tracing::{error, info, warn};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

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

    // The node release this node runs, when merod was told it
    // (`MERO_TEE_VERSION`). A namespace that admits TEEs by signed release
    // checks the quote against that release's signed measurements; without it
    // only a namespace with measurement lists can admit this node.
    let release_version = state.tee_release_version.clone();

    #[cfg(feature = "mock-attestation")]
    let mock_tee = state.mock_tee;
    #[cfg(not(feature = "mock-attestation"))]
    let mock_tee = false;
    let prompt = match super::prompt::build(
        &state.store,
        &ns_id,
        our_public_key,
        release_version.as_deref(),
        req.admitter_addrs.clone(),
        mock_tee,
    ) {
        Ok(prompt) => prompt,
        Err(err) => {
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: err.message().to_owned(),
            }
            .into_response();
        }
    };

    // Reported because the caller cannot derive it: membership is recorded
    // against the ACCOUNT a key speaks for, so a caller holding only
    // `public_key` has nothing it can match a member listing against, and no
    // endpoint maps one to the other from outside the node that owns it.
    let account_id = prompt.params.account.statement.account;

    let payloads = [prompt.payload];
    let params = prompt.params;

    if let Err(err) = state.node_client.subscribe_namespace(group_id_bytes).await {
        error!(error=?err, "Failed to subscribe to namespace topic");
        return ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to subscribe to namespace".to_owned(),
        }
        .into_response();
    }

    // Tell the node it is waiting to be admitted, and ask the named admitters
    // directly, the way an invitation's joiner does.
    //
    // The node answers a challenge only while it is registered here, so this
    // comes before the first prompt. A quote cannot be made ahead of time: the
    // admitter chooses the challenge it must carry, and the node attests when
    // it has one. With addresses, the node dials each admitter, asks for a
    // challenge and answers it, and gets a verdict back that names why it was
    // refused. Without them, or when none admits, it relies on the prompt below:
    // a member that hears it offers a challenge of its own.
    //
    // Admission here is not the end of the job: this node still holds no
    // governance state and no key, so pull right away rather than waiting a
    // poll cycle. The loop below then confirms membership and joins contexts
    // exactly as it does after a prompted admission.
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
        Err(err) => info!(
            group_id = %req.group_id,
            error = %format!("{err:#}"),
            "no admitter admitted this node directly; relying on the prompt"
        ),
    }

    // Prompt the namespace up front. A single publish at fleet-join time is
    // lost forever if it lands in an empty gossipsub mesh (no replay), which is
    // the common case for a NAT'd/relay owner whose mesh forms only
    // intermittently. The admission loop below therefore RE-prompts every poll
    // cycle until admitted or the deadline, so a *later* mesh window still
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
    // (a subscription with no chance of a prompt is useless).
    if let Err(err) = publish_prompts(&state.node_client, group_id_bytes, &payloads).await {
        if calimero_network_primitives::client::is_no_peers_subscribed_error(&err) {
            info!(
                group_id = %req.group_id,
                "First prompt hit an empty gossipsub mesh (no peers subscribed yet); \
                 deferring to the re-prompt loop, which republishes once the mesh forms"
            );
        } else {
            warn!(error=?err, "Failed to broadcast, unsubscribing from namespace");
            let _ = state
                .node_client
                .unsubscribe_namespace(group_id_bytes)
                .await;
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to broadcast the admission prompt".to_owned(),
            }
            .into_response();
        }
    }

    info!(
        group_id = %req.group_id,
        %our_public_key,
        "TeeAdmissionPrompt broadcast; re-prompting until admission then joining contexts"
    );

    // Poll for group admission, then auto-join all contexts in the namespace.
    //
    // Re-prompt strategy: this loop both (a) checks for admission and (b)
    // re-publishes the prompt each cycle the node is not yet admitted. The
    // re-prompt is request-scoped (bounded by `MAX_ADMISSION_WAIT`) rather
    // than a long-lived background task: the mdma sidecar already re-polls
    // should-join and re-invokes fleet-join, so each call covering one mesh
    // window is sufficient, and a request-scoped loop needs no extra actor /
    // lifecycle management. See the handler-level rationale comment.
    let mut contexts_joined = Vec::new();
    let mut admitted = false;
    let mut auto_follow_enabled = false;

    // Overall bound for one fleet-join call. The sidecar re-invokes across a
    // larger window, so this only needs to cover a single mesh-formation
    // attempt comfortably.
    const MAX_ADMISSION_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
    // Interval between admission checks AND between re-prompts — short enough
    // that a transient mesh window (mesh peers appear, then vanish) is hit by a
    // fresh publish, but not so tight it spams the topic.
    const ADMISSION_POLL: std::time::Duration = std::time::Duration::from_secs(2);

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

                for entry in &entries {
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
                match our_account {
                    None => warn!(
                        %our_public_key,
                        "fleet-join: this replica's key is bound to no account; skipping \
                         auto-follow self-enable. Admission succeeded, but subsequent \
                         contexts will not auto-join."
                    ),
                    Some(our_account) => match calimero_governance_store::sign_apply_and_publish(
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
                            info!(
                                group_id = %req.group_id,
                                "fleet-join: auto-follow enabled for self"
                            );
                            auto_follow_enabled = true;
                        }
                        Err(err) => warn!(
                            group_id = %req.group_id,
                            ?err,
                            "fleet-join: failed to enable auto-follow — admission succeeded but \
                             subsequent contexts will NOT auto-join until the op is retried. \
                             Operators can re-trigger fleet-join or publish MemberSetAutoFollow."
                        ),
                    },
                }

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

                // Re-prompt AFTER the poll sleep, and only if we're still
                // before the deadline. Doing it here (rather than before the
                // sleep) avoids both a duplicate publish fired back-to-back with
                // the up-front one at t=0 and a wasted publish right as we give
                // up. A single up-front publish is lost if the mesh was empty at
                // fleet-join (gossipsub does not replay), so re-publishing each
                // cycle delivers a fresh copy to a mesh window that opens later.
                // Best effort — a transport error here is logged, not fatal.
                if tokio::time::Instant::now() < deadline {
                    match publish_prompts(&state.node_client, group_id_bytes, &payloads).await {
                        Ok(mesh_peers) => tracing::debug!(
                            group_id = %req.group_id,
                            mesh_peers,
                            "re-prompted the namespace while awaiting admission"
                        ),
                        Err(reprompt_err) => warn!(
                            group_id = %req.group_id,
                            error = ?reprompt_err,
                            "re-prompt publish failed; will retry next cycle"
                        ),
                    }

                    // Bootstrap pull: a bare prompter holds NO namespace
                    // governance state (it only `subscribe_namespace`'d to send
                    // the prompt). Once the verifier admits it, the verifier
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
                    // `now < deadline` check as the re-prompt because it is a
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

    ApiResponse {
        payload: serde_json::json!({
            "status": if admitted { "joined" } else { "announced" },
            "group_id": req.group_id,
            "namespace_id": hex::encode(ns_id.to_bytes()),
            "public_key": our_public_key.to_string(),
            "account": hex::encode(account_id.as_bytes()),
            "admitted": admitted,
            "auto_follow_enabled": auto_follow_enabled,
            "contexts_joined": contexts_joined,
        }),
    }
    .into_response()
}

/// Publish each prompt on the namespace topic, stopping at the first failure.
/// Returns the mesh size the last publish saw.
async fn publish_prompts(
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
