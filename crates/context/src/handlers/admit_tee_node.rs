use std::cmp::Ordering;
use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{AdmitTeeNodeRequest, TeeAdmissionOutcome};
use calimero_context_client::local_governance::{AckRouter, GroupOp, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use calimero_tee_release::{compare_release_versions, NodeRelease, NODE_RELEASE_TAG_PREFIX};
use tracing::{debug, warn};

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{
    GroupKeyring, MembershipPolicy, MembershipRepository, NamespaceRepository, TeeAdmissionPolicy,
    TeeAdmissionPolicyRead, TeeReleaseTrust,
};

/// Publish a `RootOp::KeyDelivery` wrapping the namespace group key for
/// `member`, signed with the verifier's namespace identity (`signer_sk`).
///
/// The TEE-attestation join op is an encrypted `NamespaceOp::Group` that
/// only the verifier can apply, so the admitted node can't decrypt its own
/// membership until it holds the key. The verifier (which holds the key)
/// records the delivery here — one-shot, admin-initiated. Idempotent on any
/// member that applies it: a duplicate `KeyDelivery` for a key already held is
/// a no-op (`store_key` keys by content).
///
/// The op is SEALED under the namespace key, so an admitted node that holds no
/// namespace key yet — which is every node this path admits — cannot open it.
/// For that node the transfer is the joiner-side pull
/// (`recover_missing_group_keys`), not this op; publishing it keeps the
/// members-only causal record of when the key was handed over, and keeps who
/// was admitted when off the namespace topic.
async fn deliver_group_key_to_member(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    ack_router: &AckRouter,
    group_id: &ContextGroupId,
    signer_sk: &PrivateKey,
    member: &PublicKey,
) -> eyre::Result<()> {
    let namespace_id = NamespaceRepository::new(store).resolve(group_id)?;

    let Some((_key_id, group_key)) = GroupKeyring::new(store, *group_id).load_current_key()? else {
        // The verifier admitted the node against this namespace's policy but
        // holds no group key for it — there is nothing to deliver. This should
        // not happen for a namespace owner/admin, but bail loudly rather than
        // silently leave the admitted node un-bootstrappable.
        eyre::bail!("verifier has no group key for namespace; cannot deliver to admitted TEE node");
    };

    let envelope =
        GroupKeyring::wrap_for_member(signer_sk, member, &group_id.to_bytes(), &group_key)?;

    let delivery_op = calimero_governance_store::seal_root_op_for_publish(
        store,
        namespace_id.to_bytes().into(),
        RootOp::KeyDelivery {
            group_id: group_id.to_bytes().into(),
            envelope,
        },
    )?;

    // `required_signers` is None. It used to name the admitted member, so the
    // report's `acked_by` read as "the delivery landed" — but a sealed op cannot
    // be applied by a node that holds no namespace key, which is precisely the
    // node being admitted. Requiring its ack would fail the publish report on
    // every successful admission. Confirmation moves to the pull, which is where
    // the key now actually arrives.
    let report = calimero_governance_store::sign_and_publish_namespace_op(
        store,
        node_client,
        ack_router,
        namespace_id.to_bytes().into(),
        signer_sk,
        delivery_op,
        None,
    )
    .await?;

    debug!(
        group_id = %hex::encode(group_id.to_bytes()),
        %member,
        acked = report.acked_by.len(),
        elapsed_ms = report.elapsed_ms,
        "published KeyDelivery for admitted TEE node"
    );

    Ok(())
}

/// Check a quote's registers against a list policy's allowlists.
fn check_measurement_lists(
    policy: &TeeAdmissionPolicy,
    mrtd: &str,
    rtmr0: &str,
    rtmr1: &str,
    rtmr2: &str,
    rtmr3: &str,
) -> eyre::Result<()> {
    if policy.allowed_mrtd.is_empty() {
        eyre::bail!(
            "TEE admission policy has empty allowed_mrtd — at least one MRTD must be specified"
        );
    }
    if !policy.allowed_mrtd.iter().any(|a| a == mrtd) {
        eyre::bail!("MRTD not in policy allowlist");
    }
    if !policy.allowed_rtmr0.is_empty() && !policy.allowed_rtmr0.iter().any(|a| a == rtmr0) {
        eyre::bail!("RTMR0 not in policy allowlist");
    }
    // RTMR1 and RTMR2 are MANDATORY, for RTMR3's sake (see below). RTMR3 is
    // extended from public inputs, so it only proves which image ran if the
    // kernel (RTMR1) and the command line + initrd (RTMR2) that ran before
    // `calimero-init` are pinned too; otherwise a custom kernel or initrd
    // can extend RTMR3 with a locked profile's string. RTMR0 (the VM's
    // hardware configuration) stays optional.
    if policy.allowed_rtmr1.is_empty() || policy.allowed_rtmr2.is_empty() {
        eyre::bail!(
            "TEE admission policy has an empty allowed_rtmr1 or allowed_rtmr2 — both must be \
             specified. RTMR3 is extended from public inputs, so it only identifies the image \
             when the kernel (RTMR1) and command line + initrd (RTMR2) are pinned too. Take \
             the values from the release's published-mrtds.json."
        );
    }
    if !policy.allowed_rtmr1.iter().any(|a| a == rtmr1) {
        eyre::bail!("RTMR1 not in policy allowlist");
    }
    if !policy.allowed_rtmr2.iter().any(|a| a == rtmr2) {
        eyre::bail!("RTMR2 not in policy allowlist");
    }
    // RTMR3 IS MANDATORY, and it is the only field that pins the image.
    //
    // `allowed_mrtd` above cannot do it. MRTD measures the virtual firmware,
    // so it is identical across every PROFILE of a release and stays
    // constant across RELEASES -- `locked-read-only` reported the same
    // c1ee9c16… for 2.3.62, 2.3.63 and 2.3.65, and every profile of each.
    // A policy naming only an MRTD admits a `debug` image, which carries no
    // lockdown role: openssh-server, the serial console and the rescue shell
    // are present and root is not locked.
    //
    // `calimero-init` extends RTMR3 with
    // `calimero-rtmr3-v2:<role>:<profile>:<root_hash>`, so it names exactly
    // one (profile, release) pair. The cost is that it CHANGES EVERY
    // RELEASE, which is why this was optional: pinning it means the policy
    // must gain the new value before nodes on a new image can join. That is
    // the intended trade -- an allowlist that silently stops narrowing is
    // worse than one that has to be maintained.
    //
    // Empty is a refusal, not a skip. Under the old `is_empty()` guard an
    // empty list meant "do not check", so the weakest policy was the one
    // that looked like it had simply not been filled in.
    if policy.allowed_rtmr3.is_empty() {
        eyre::bail!(
            "TEE admission policy has empty allowed_rtmr3 — at least one RTMR3 must be \
             specified. MRTD does not identify the image: it is the same for every profile \
             of a release and does not change between most releases, so a policy without \
             RTMR3 admits any profile, including debug images that are not locked down. \
             RTMR3 is published per profile in the release's published-mrtds.json and \
             changes each release, so add the new value when upgrading."
        );
    }
    if !policy.allowed_rtmr3.iter().any(|a| a == rtmr3) {
        eyre::bail!("RTMR3 not in policy allowlist");
    }
    Ok(())
}

/// The release a TEE claims to run, accepted as far as it can be without
/// fetching it: it must be named, and be no older than the policy's floor.
struct SignedReleaseClaim {
    trust: TeeReleaseTrust,
    version: String,
}

fn signed_release_claim(
    trust: TeeReleaseTrust,
    release_version: Option<&str>,
) -> eyre::Result<SignedReleaseClaim> {
    let Some(claimed) = release_version else {
        eyre::bail!(
            "the namespace admits TEEs by signed release, and this node did not name the \
             mero-tee release it runs; it needs a build that sends one"
        );
    };
    let version =
        calimero_tee_release::normalize_release_version(claimed, NODE_RELEASE_TAG_PREFIX)?;
    if let Some(floor) = trust.min_release_version.as_deref() {
        match compare_release_versions(&version, floor) {
            Some(Ordering::Less) => {
                eyre::bail!("mero-tee release {version} is older than the policy's minimum {floor}")
            }
            Some(_) => {}
            None => eyre::bail!(
                "mero-tee release {version} cannot be compared with the policy's minimum {floor}"
            ),
        }
    }
    Ok(SignedReleaseClaim { trust, version })
}

impl SignedReleaseClaim {
    /// Fetch the claimed release's signed measurements and name the allowed
    /// profile the quote's registers match.
    ///
    /// The signature is what makes this a check: `published-mrtds.json` is
    /// accepted only if the `Release mero-tee` workflow signed it, so a TEE
    /// that names a release it does not run, or a release that was never
    /// published, is refused here.
    async fn verify(
        &self,
        mrtd: &str,
        rtmr0: &str,
        rtmr1: &str,
        rtmr2: &str,
        rtmr3: &str,
    ) -> eyre::Result<String> {
        let release = calimero_tee_release::fetch_node_release(&self.version)
            .await
            .map_err(|err| {
                eyre::eyre!(
                    "could not verify mero-tee release {}'s signed measurements: {err:#}",
                    self.version
                )
            })?;
        matched_profile(&self.trust, &release, mrtd, rtmr0, rtmr1, rtmr2, rtmr3)
    }
}

fn matched_profile(
    trust: &TeeReleaseTrust,
    release: &NodeRelease,
    mrtd: &str,
    rtmr0: &str,
    rtmr1: &str,
    rtmr2: &str,
    rtmr3: &str,
) -> eyre::Result<String> {
    release
        .matching_profile(&trust.allowed_profiles, mrtd, rtmr0, rtmr1, rtmr2, rtmr3)
        .map(str::to_owned)
        .ok_or_else(|| {
            eyre::eyre!(
                "the quote's measurements match none of the allowed profiles ({}) of signed \
                 mero-tee release {}",
                trust.allowed_profiles.join(", "),
                release.version
            )
        })
}

/// Publish a `TeeAuthorityEvidence` op for an admitted TEE on the namespace root.
///
/// Peers verify it offline at apply, which is what lets them treat the TEE as
/// the TEE authority without trusting this node's check of its quote.
#[expect(
    clippy::too_many_arguments,
    reason = "one publish, each argument distinct"
)]
async fn publish_authority_evidence(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    ack_router: &AckRouter,
    group_id: &ContextGroupId,
    signer_sk: &PrivateKey,
    member: calimero_account::AccountId,
    attested_key: PublicKey,
    evidence: calimero_context_client::group::TeeAuthorityEvidencePayload,
) -> eyre::Result<()> {
    let root = NamespaceRepository::new(store).resolve(group_id)?;
    let report = calimero_governance_store::sign_apply_and_publish(
        store,
        node_client,
        ack_router,
        &root,
        signer_sk,
        GroupOp::TeeAuthorityEvidence {
            member,
            attested_key,
            quote: evidence.quote,
            collateral: evidence.collateral,
            attested_at: evidence.attested_at,
        },
    )
    .await?;
    report.observe("admit_tee_node", "TeeAuthorityEvidence");
    debug!(%attested_key, "published TEE authority evidence");
    Ok(())
}

impl Handler<AdmitTeeNodeRequest> for ContextManager {
    type Result = ActorResponse<Self, <AdmitTeeNodeRequest as Message>::Result>;

    fn handle(
        &mut self,
        AdmitTeeNodeRequest {
            group_id,
            member,
            account,
            quote_hash,
            mrtd,
            rtmr0,
            rtmr1,
            rtmr2,
            rtmr3,
            tcb_status,
            is_mock,
            release_version,
            evidence,
        }: AdmitTeeNodeRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let (signer, node_sk) = match self.resolve_signer(&group_id) {
            Ok(pair) => pair,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // Every member node receives the announce, but only an admin or an
        // already-admitted TEE may vouch for it — peers refuse the op from
        // anyone else (`require_tee_attestation_verifier`). Stand down here,
        // before publishing an op that could never apply anywhere. Not an
        // error: on most member nodes this is the expected outcome, and the
        // admission is left to a node that may vouch.
        let signer_account =
            match crate::member_account::require(&self.datastore, &group_id, &signer) {
                Ok(account) => account,
                Err(err) => return ActorResponse::reply(Err(err)),
            };
        match MembershipPolicy::new(&self.datastore, group_id)
            .is_tee_attestation_verifier(&signer_account)
        {
            Ok(true) => {}
            Ok(false) => {
                debug!(
                    %member,
                    ?group_id,
                    "not an admin or admitted TEE here; leaving the TEE admission to one"
                );
                return ActorResponse::reply(Ok(TeeAdmissionOutcome::NotAVoucher));
            }
            Err(err) => return ActorResponse::reply(Err(err)),
        }

        let policy = match calimero_governance_store::read_tee_admission_policy(
            &self.datastore,
            &group_id,
        ) {
            Ok(TeeAdmissionPolicyRead::Set(p)) => p,
            Ok(TeeAdmissionPolicyRead::NotSet) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "no TeeAdmissionPolicy set for group"
                )))
            }
            // Refusing either way, but the operator is told which fault they
            // have. "No policy set" sends someone who HAS set one off to set
            // it again; the real problem is an op-log entry nobody can read.
            Ok(TeeAdmissionPolicyRead::Unreadable { undecodable }) => {
                let detail = undecodable
                    .iter()
                    .map(|e| format!("seq {}: {}", e.sequence, e.error))
                    .collect::<Vec<_>>()
                    .join("; ");
                return ActorResponse::reply(Err(eyre::eyre!(
                    "TeeAdmissionPolicy could not be read: {} op-log entr{} do not decode \
                     ({detail}). A policy may well be set — this is not the same as no policy \
                     being set.",
                    undecodable.len(),
                    if undecodable.len() == 1 { "y" } else { "ies" },
                )));
            }
            Err(e) => return ActorResponse::reply(Err(e)),
        };

        if is_mock && !policy.accept_mock {
            return ActorResponse::reply(Err(eyre::eyre!(
                "mock attestation rejected by group policy"
            )));
        }

        // Fail-closed TCB-status gate (audit #356 / #17). An empty
        // `allowed_tcb_statuses` no longer skips the check: it enforces against
        // the secure default `{UpToDate}`. `Revoked` is rejected
        // unconditionally. Mock (`is_mock`) bypasses the allowlist only when the
        // policy sets `accept_mock` — defense-in-depth alongside the explicit
        // `is_mock && !accept_mock` rejection above. Shared with the op-apply
        // path (`governance_store::membership::tcb_status_allowed`).
        if !calimero_governance_store::tcb_status_allowed(
            &policy.allowed_tcb_statuses,
            &tcb_status,
            is_mock,
            policy.accept_mock,
        ) {
            return ActorResponse::reply(Err(eyre::eyre!("TCB status not in policy allowlist")));
        }

        // A signed-release policy names no measurements: they come from the
        // release the TEE says it runs, fetched and signature-checked below,
        // outside the actor, once it is known this is a new admission. A list
        // policy is checked here, as it always was.
        if policy.release_trust.is_none() {
            if let Err(err) =
                check_measurement_lists(&policy, &mrtd, &rtmr0, &rtmr1, &rtmr2, &rtmr3)
            {
                return ActorResponse::reply(Err(err));
            }
        }

        // Direct-row check: TEE admission writes the node's direct
        // membership row + signing key. An inherited match via the
        // Open-subgroup chain (#2256) does not mean the node already has
        // its own direct row, and skipping the write here would leave
        // the TEE without a per-node row that subsequent direct-membership
        // operations expect.
        // The request names the replica's KEY (the key delivery below is an ECDH
        // wrap and can only be addressed to one); the membership row it is about
        // to write names the account that key acts as.
        //
        // When a credential rides along, IT is the answer — and it has to be.
        // That is the root-admission case: the replica is not bound here yet,
        // and the very op this handler is about to publish is what binds it.
        // Resolving from the rows first would refuse every first admission, and
        // the binding would never be written because the op is never sent.
        // Without one, the admission is an already-bound namespace member moving
        // inward, so the rows are the answer.
        let member_account = match account.as_ref() {
            Some(credential) => credential.statement.account,
            None => match crate::member_account::require(&self.datastore, &group_id, &member) {
                Ok(account) => account,
                Err(err) => return ActorResponse::reply(Err(err)),
            },
        };
        let already_member = match MembershipRepository::new(&self.datastore)
            .has_direct_member(&group_id, &member_account)
        {
            Ok(already) => already,
            Err(e) => return ActorResponse::reply(Err(e)),
        };
        if already_member {
            // Admitted before. Its re-announcement is the chance to publish
            // evidence that never landed, or to replace evidence old enough to
            // be due for a refresh, so a TEE keeps its authority past the first
            // evidence's lifetime.
            let refresh_due = match calimero_governance_store::tee_evidence_refresh_due(
                &self.datastore,
                &group_id,
                &member_account,
            ) {
                Ok(due) => due,
                Err(e) => return ActorResponse::reply(Err(e)),
            };
            let Some(evidence) = evidence.filter(|_| refresh_due) else {
                return ActorResponse::reply(Ok(TeeAdmissionOutcome::AlreadyMember));
            };
            let datastore = self.datastore.clone();
            let node_client = self.node_client.clone();
            let ack_router = Arc::clone(&self.ack_router);
            return ActorResponse::r#async(
                async move {
                    publish_authority_evidence(
                        &datastore,
                        &node_client,
                        &ack_router,
                        &group_id,
                        &PrivateKey::from(node_sk),
                        member_account,
                        member,
                        evidence,
                    )
                    .await?;
                    Ok(TeeAdmissionOutcome::AlreadyMember)
                }
                .into_actor(self),
            );
        }

        // After the already-member branch, which admits nothing: a TEE admitted
        // earlier re-announces only to have its evidence published (the
        // server's `tee::evidence_retry`), and that announce names no release.
        //
        // A mock quote carries made-up registers no release publishes, so it
        // is judged on `accept_mock` alone, the rule the list form applies.
        // A subgroup admission (`account` is `None`) moves a namespace member
        // inward: its release was checked when the root admitted it, and the
        // record it is re-admitted from does not carry the version.
        let release_claim = match policy.release_trust {
            Some(trust) if !is_mock && account.is_some() => {
                match signed_release_claim(trust, release_version.as_deref()) {
                    Ok(claim) => Some(claim),
                    Err(err) => return ActorResponse::reply(Err(err)),
                }
            }
            _ => None,
        };

        match calimero_governance_store::is_quote_hash_used(&self.datastore, &group_id, &quote_hash)
        {
            Ok(true) => {
                return ActorResponse::reply(Err(eyre::eyre!("TEE attestation quote already used")))
            }
            Ok(false) => {}
            Err(e) => return ActorResponse::reply(Err(e)),
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        // The fallback this used to have read the same key back out of a per-group
        // store that the line above had just written it into. One key, held here.
        let effective_signing_key = node_sk;

        ActorResponse::r#async(
            async move {
                if let Some(claim) = &release_claim {
                    let profile = claim.verify(&mrtd, &rtmr0, &rtmr1, &rtmr2, &rtmr3).await?;
                    debug!(
                        %member,
                        ?group_id,
                        release = %claim.version,
                        %profile,
                        "TEE matches a signed mero-tee release"
                    );
                }
                let sk = PrivateKey::from(effective_signing_key);
                // Two forms, one decision: does this admission have to carry a
                // credential?
                //
                // A FLEET replica is an outsider joining the namespace, so its
                // device has to be bound in the same apply as its membership —
                // and that means peers must be able to read the credential,
                // which means cleartext. A SUBGROUP admission is an existing
                // namespace member moving inward; bindings are namespace-keyed,
                // so it is already bound and the encrypted group op stays
                // correct for it.
                let report = if let Some(account) = account {
                    let namespace_id =
                        calimero_governance_store::NamespaceRepository::new(&datastore)
                            .resolve(&group_id)?;
                    // Sealed. The ADMITTER publishes this, and an admitter holds
                    // the namespace key by definition, so the seal costs nothing
                    // here. The TEE node it admits does not hold that key -- it
                    // reads its own admission only after the pull hands it one,
                    // the same order `KeyDelivery` was put on in #3847. What
                    // sealing removes is every non-member on the namespace topic
                    // being able to read which fleet replica was admitted, with
                    // its attestation measurements, and when.
                    let op = calimero_governance_store::seal_root_op_for_publish(
                        &datastore,
                        namespace_id.to_bytes().into(),
                        RootOp::MemberJoinedViaTeeAttestation {
                            group_id,
                            member,
                            quote_hash,
                            mrtd,
                            rtmr0,
                            rtmr1,
                            rtmr2,
                            rtmr3,
                            tcb_status,
                            role: GroupMemberRole::ReadOnlyTee,
                            account,
                        },
                    )?;
                    // The namespace publisher always returns a report; the group
                    // one returns `Option` because a group op can be applied
                    // without a publish. Normalise to the wider shape.
                    Some(
                        calimero_governance_store::sign_apply_and_publish_namespace_op(
                            &datastore,
                            &node_client,
                            &ack_router,
                            namespace_id.to_bytes().into(),
                            &sk,
                            op,
                        )
                        .await?,
                    )
                } else {
                    calimero_governance_store::sign_apply_and_publish(
                        &datastore,
                        &node_client,
                        &ack_router,
                        &group_id,
                        &sk,
                        GroupOp::MemberJoinedViaTeeAttestation {
                            // The encrypted form names an ACCOUNT: a subgroup
                            // admission moves an existing namespace member
                            // inward, so it is already bound and resolvable.
                            member: member_account,
                            quote_hash,
                            mrtd,
                            rtmr0,
                            rtmr1,
                            rtmr2,
                            rtmr3,
                            tcb_status,
                            role: GroupMemberRole::ReadOnlyTee,
                        },
                    )
                    .await?
                };
                report.observe("admit_tee_node", "MemberJoinedViaTeeAttestation");

                // After the admission, so peers apply it first. A failure is
                // logged, not returned: the TEE is admitted either way, and while
                // authorship is on it keeps re-announcing until some admitter
                // publishes the evidence (the server's `tee::evidence_retry`).
                if let Some(evidence) = evidence {
                    if let Err(err) = publish_authority_evidence(
                        &datastore,
                        &node_client,
                        &ack_router,
                        &group_id,
                        &sk,
                        member_account,
                        member,
                        evidence,
                    )
                    .await
                    {
                        warn!(
                            %member,
                            ?err,
                            "TEE admitted, but publishing its authority evidence failed; the \
                             TEE re-announces while authorship is on, which retries it"
                        );
                    }
                }

                debug!(%member, ?group_id, "TEE node admitted via attestation");

                // Deliver the namespace group key to the freshly-admitted
                // TEE node. A `MemberJoinedViaTeeAttestation` op is an
                // encrypted `NamespaceOp::Group` only the verifier can apply,
                // so the node can't decrypt its own membership until it holds
                // the key. The verifier holds it and signs with its namespace
                // identity, so it publishes the `KeyDelivery` directly (the
                // namespace governance DAG after fleet-join, though it cannot
                // read this op itself — see `deliver_group_key_to_member`).
                // Best-effort: the joiner-side pull is what actually hands the
                // admitted node the key.
                if let Err(err) = deliver_group_key_to_member(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &group_id,
                    &sk,
                    &member,
                )
                .await
                {
                    warn!(
                        %member,
                        ?group_id,
                        ?err,
                        "TEE admission succeeded but KeyDelivery to the admitted node \
                         failed — the node recovers it via the joiner-side pull, or on \
                         re-admission."
                    );
                }

                // Auto-follow flags for the admitted TEE member are
                // published by the member itself in `fleet_join.rs` after
                // it observes admission — signed with its own namespace
                // identity, which satisfies `MemberSetAutoFollow`'s
                // admin-or-self authorization rule. The verifier (this
                // handler) has neither admin authority nor the member's
                // signing key, so it can't do it here.

                Ok(TeeAdmissionOutcome::Admitted)
            }
            .into_actor(self),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use calimero_tee_release::ProfileMeasurements;

    use super::*;

    fn trust(min: Option<&str>) -> TeeReleaseTrust {
        TeeReleaseTrust {
            allowed_profiles: vec!["locked-read-only".to_owned()],
            min_release_version: min.map(str::to_owned),
        }
    }

    fn release() -> NodeRelease {
        let one = |v: &str| vec![v.to_owned()];
        let profile = |rtmr3: &str| ProfileMeasurements {
            allowed_mrtd: one("mrtd"),
            allowed_rtmr0: vec![],
            allowed_rtmr1: one("rtmr1"),
            allowed_rtmr2: one("rtmr2"),
            allowed_rtmr3: one(rtmr3),
        };
        NodeRelease {
            version: "2.3.72".to_owned(),
            profiles: BTreeMap::from([
                ("locked-read-only".to_owned(), profile("locked")),
                ("debug".to_owned(), profile("debug")),
            ]),
        }
    }

    #[test]
    fn a_signed_release_policy_needs_the_release_named() {
        let err = signed_release_claim(trust(None), None).err().unwrap();
        assert!(err.to_string().contains("did not name"), "{err}");
    }

    #[test]
    fn the_claimed_release_is_normalised_and_held_to_the_floor() {
        let claim = signed_release_claim(trust(Some("2.3.72")), Some("mero-tee-v2.3.72")).unwrap();
        assert_eq!(claim.version, "2.3.72");
        assert!(signed_release_claim(trust(Some("2.3.72")), Some("2.3.80")).is_ok());

        let err = signed_release_claim(trust(Some("2.3.72")), Some("2.3.71"))
            .err()
            .unwrap();
        assert!(err.to_string().contains("older than"), "{err}");
        assert!(
            signed_release_claim(trust(None), Some("latest")).is_err(),
            "a claim that is not a version is refused before any fetch"
        );
    }

    #[test]
    fn only_an_allowed_profile_of_the_release_admits() {
        let release = release();
        let profile = matched_profile(
            &trust(None),
            &release,
            "mrtd",
            "rtmr0",
            "rtmr1",
            "rtmr2",
            "locked",
        )
        .unwrap();
        assert_eq!(profile, "locked-read-only");

        let err = matched_profile(
            &trust(None),
            &release,
            "mrtd",
            "rtmr0",
            "rtmr1",
            "rtmr2",
            "debug",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("locked-read-only"),
            "a debug image of a signed release is still not an allowed profile: {err}"
        );
    }
}
