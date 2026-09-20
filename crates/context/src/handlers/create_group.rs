use std::sync::Arc;

use actix::{ActorFutureExt, ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{CreateGroupRequest, CreateGroupResponse};
use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp};
use calimero_context_config::types::{BytecodeId, ContextGroupId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::key::{GroupMetaValue, GroupTarget};
use calimero_store::types::ApplicationMeta as ApplicationMetaValue;
use calimero_store::Store;
use rand::RngExt;
use tracing::{debug, info, warn};

use crate::ContextManager;
use calimero_governance_store;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{
    CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository, MetadataRepository,
};

impl Handler<CreateGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <CreateGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        CreateGroupRequest {
            group_id,
            bytecode_id,
            application_id,
            name,
            parent_group_id,
            restricted,
        }: CreateGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let group_id = group_id.unwrap_or_else(|| {
            let bytes: [u8; 32] = rand::rng().random();
            bytes.into()
        });

        if let Ok(Some(_)) = MetaRepository::new(&self.datastore).load(&group_id) {
            return ActorResponse::reply(Err(eyre::eyre!("group '{group_id:?}' already exists")));
        }

        let namespace_anchor_group_id = parent_group_id.as_ref().unwrap_or(&group_id);
        let (namespace_id, admin_identity, sk_bytes) =
            match self.get_or_create_namespace_identity(namespace_anchor_group_id) {
                Ok(result) => result,
                Err(err) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "failed to resolve namespace identity: {err}"
                    )))
                }
            };

        // The creator's ACCOUNT — every governance row this handler writes names
        // a principal, and a principal is an account.
        //
        // A namespace root has no bindings yet (nothing has been applied), so the
        // founder's account comes from the credential this node mints for itself;
        // the genesis op carries that same credential and the apply records the
        // binding, which is what makes the founder resolvable to every peer
        // afterwards. A subgroup is different: its creator is already a namespace
        // member, so its account MUST come from the binding rows — deriving it
        // locally instead would let a node whose root was replaced write rows its
        // peers resolve to somebody else.
        //
        // Minted only for a root. A subgroup never uses the credential, so
        // building one for it would let an unrelated fault — a namespace
        // identity or account root in some transient state — refuse a creation
        // that had no need of it.
        let (admin_account, founder_credential) = if parent_group_id.is_some() {
            let account = match calimero_governance_store::member_account_in_namespace(
                &self.datastore,
                &namespace_id,
                &admin_identity,
            ) {
                Ok(Some(account)) => account,
                Ok(None) => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "cannot create a subgroup: this node's identity is bound to no account \
                         in namespace '{namespace_id:?}'"
                    )))
                }
                Err(err) => return ActorResponse::reply(Err(err)),
            };
            (account, None)
        } else {
            match crate::join_credential::build(&self.datastore, &namespace_id, &admin_identity) {
                Ok(credential) => (credential.statement.account, Some(credential)),
                Err(err) => {
                    return ActorResponse::reply(Err(
                        err.wrap_err("failed to mint this node's account credential")
                    ))
                }
            }
        };

        // Subgroups inherit target_application_id from the parent (namespace root owns the app).
        let effective_application_id = if let Some(ref parent_id) = parent_group_id {
            let parent_meta = match MetaRepository::new(&self.datastore).load(parent_id) {
                Ok(Some(m)) => m,
                _ => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "parent group '{parent_id:?}' not found"
                    )));
                }
            };
            // Authorization. Namespace-root admins may create a subgroup at
            // any depth. A non-admin namespace member may create one *directly
            // under the namespace root* if they hold `CAN_CREATE_SUBGROUP`
            // (honored only at root level — see the capability's doc and
            // `execute_group_created`, which re-checks this on every peer).
            let is_namespace_admin = match MembershipRepository::new(&self.datastore)
                .is_admin(&namespace_id, &admin_account)
            {
                Ok(v) => v,
                Err(err) => return ActorResponse::reply(Err(err)),
            };
            if !is_namespace_admin {
                if *parent_id != namespace_id {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "creating a subgroup under non-root parent '{parent_id:?}' requires \
                         namespace admin (delegated nested-subgroup creation is not yet supported)"
                    )));
                }
                if let Err(err) =
                    calimero_governance_store::PermissionChecker::new(&self.datastore, *parent_id)
                        .require_can_create_subgroup(&admin_identity)
                {
                    return ActorResponse::reply(Err(err));
                }
            }
            Some(parent_meta.target.application_id)
        } else {
            application_id
        };

        // Derive bytecode_id from the resolved application's bytecode blob_id
        // when the caller didn't provide one. This is the same value that
        // `set_target_application` (upgrade_group's apply path) writes after
        // an upgrade, so the cascade predicate (from_bytecode_id == descendant
        // bytecode_id) walks into freshly-created subgroups without needing a
        // pre-cascade alignment upgrade. A randomly-seeded bytecode_id, which
        // is what this used to do, made every cascade silently skip the
        // descendant subtree.
        //
        // A caller-provided bytecode_id pins the group to a specific version;
        // it is verified inside the async block below (blob present locally
        // + manifest package matches the row's package).
        //
        // A root that targets nothing, the account namespace, keeps the unset target.
        let target = match effective_application_id {
            Some(application_id) => match load_app_meta(&self.datastore, &application_id) {
                Ok(app_meta) => GroupTarget {
                    application_id,
                    bytecode_id: *app_meta.bytecode.blob_id().as_ref(),
                    package: app_meta.package,
                    version: app_meta.version,
                },
                Err(err) => return ActorResponse::reply(Err(err)),
            },
            None => GroupTarget::default(),
        };
        let requested_bytecode_id = bytecode_id;

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        // Reserve the group id SYNCHRONOUSLY, before spawning the async body.
        // The existence guard at the top of `handle` and this save both run in
        // the synchronous portion of the handler, which the actor runs to
        // completion before dequeuing the next message. Without a synchronous
        // reservation, two same-id CreateGroupRequests could both pass the
        // guard (the meta was only written later, inside the async body) and
        // each run the full create, emitting duplicate governance ops. The
        // async body overwrites this reservation with the final meta once the
        // bytecode_id is resolved/verified; the cleanup map on the returned future
        // deletes it if that async work fails, so a failed create frees the id
        // for a clean retry rather than wedging it behind the guard forever.
        let reservation_now = match std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
        {
            Ok(d) => d.as_secs(),
            Err(err) => {
                warn!(
                    %err,
                    ?group_id,
                    "system clock is before UNIX_EPOCH; stamping created_at=0 on the group reservation"
                );
                0
            }
        };
        let reservation_meta = GroupMetaValue {
            // The reservation only holds the id slot; use the verified
            // application-row blob, never the caller's still-unverified
            // `requested_bytecode_id`. The async body overwrites this with the
            // final, verified target on success.
            target: target.clone(),
            created_at: reservation_now,
            admin_identity: admin_account,
            owner_identity: admin_account,
            migration: None,
            auto_join: true,
        };
        if let Err(err) = MetaRepository::new(&self.datastore).save(&group_id, &reservation_meta) {
            return ActorResponse::reply(Err(err));
        }

        ActorResponse::r#async(
            async move {
                let bytecode_id = match requested_bytecode_id {
                    Some(requested) => {
                        verify_requested_bytecode_id(
                            &node_client,
                            &requested,
                            target.bytecode_id,
                            &target.package,
                        )
                        .await?;
                        requested
                    }
                    None => BytecodeId::from(target.bytecode_id),
                };

                // Reuse the timestamp resolved (and warned-on-error) for the
                // synchronous reservation rather than recomputing it here with a
                // second silent `unwrap_or(0)`. The final meta and the
                // reservation it replaces then carry the same `created_at`.
                let meta = GroupMetaValue {
                    target: GroupTarget {
                        bytecode_id: bytecode_id.to_bytes(),
                        ..target.clone()
                    },
                    created_at: reservation_now,
                    admin_identity: admin_account,
                    // Creator is the initial Owner. Transferable via
                    // `GroupOp::TransferOwnership`.
                    owner_identity: admin_account,
                    migration: None,
                    auto_join: true,
                };

                // Write the group's local rows. If any write fails partway,
                // unwind the rows already written before returning so a retry
                // with the same id starts from a clean slate rather than a
                // half-created group (mirrors the genesis-failure rollback
                // below).
                //
                // `group_key_id` is threaded out through outer mutation (not the
                // closure's return) BECAUSE the rollback runs on the ERROR path:
                // it needs to drop the encryption key by the id actually stored,
                // or skip it if the failure preceded the key write. The success
                // path takes `key_id` straight from the closure's `Ok`.
                let mut group_key_id: Option<[u8; 32]> = None;
                let write_local_rows = (|| -> eyre::Result<[u8; 32]> {
                    MetaRepository::new(&datastore).save(&group_id, &meta)?;
                    MembershipRepository::new(&datastore).add_member(
                        &group_id,
                        &admin_account,
                        GroupMemberRole::Admin,
                    )?;

                    CapabilitiesRepository::new(&datastore).set_default_capabilities(
                        &group_id,
                        initial_default_capabilities(parent_group_id.is_none()),
                    )?;

                    // Generate and store the group encryption key.
                    //
                    // Minted for every group whatever its visibility: it is the
                    // key this group uses if it is ever `Restricted`. An
                    // Open-chain subgroup is encrypted under the NAMESPACE key
                    // instead (`calimero_governance_store::key_covering_group`),
                    // leaving this row unused until a
                    // `SubgroupVisibilitySet -> Restricted` makes it the group's
                    // real key — that flip establishes no key of its own, and an
                    // apply handler could not mint one consistently across peers.
                    // Unused is not the same as free to hand out: no reader may
                    // serve or adopt this row while the chain is Open.
                    let group_key: [u8; 32] = rand::rng().random();
                    let key_id = GroupKeyring::new(&datastore, group_id).store_key(&group_key)?;
                    group_key_id = Some(key_id);
                    tracing::debug!(
                        ?group_id,
                        key_id = %hex::encode(key_id),
                        "stored initial group key"
                    );

                    Ok(key_id)
                })();
                let key_id = match write_local_rows {
                    Ok(key_id) => key_id,
                    Err(err) => {
                        rollback_local_group_rows(
                            &datastore,
                            &group_id,
                            &admin_account,
                            group_key_id,
                        );
                        return Err(err);
                    }
                };

                // In the namespace model, group hierarchy is tracked in the
                // namespace DAG (RootOp::GroupCreated), not via parent refs.
                if let Err(err) = node_client
                    .subscribe_namespace(namespace_id.to_bytes())
                    .await
                {
                    warn!(
                        ?err,
                        namespace_id=%hex::encode(namespace_id.to_bytes()),
                        "failed to subscribe to namespace before publishing governance ops"
                    );
                }

                let signer_sk = PrivateKey::from(sk_bytes);
                // Strict-tree refactor: GroupCreated is now an atomic
                // create+nest op. It ONLY applies to subgroups — the namespace
                // root itself has no parent by definition.
                //
                // #2474: root creation (parent_group_id is None) now emits a
                // replayable `RootOp::NamespaceCreated { founder }` GENESIS op so
                // a bootstrapping replica derives the founding admin/owner
                // authoritatively from the synced DAG instead of TOFU-seeding it
                // from the KeyDelivery signer. This is the FIRST op in the
                // namespace DAG — its defining invariant is that it has NO
                // parents (the head record is empty for a brand-new namespace,
                // so `read_head_record` returns empty `parent_hashes`). Its
                // nonce is 1, not 0 (`read_head_record` defaults `next_nonce` to
                // 1 when the head is absent); `op.nonce` is informational and
                // signature-covered, but DAG sequencing comes from
                // `read_head_record().next_nonce`, never from `op.nonce`. The
                // genesis is signed+published via the same path
                // subgroup GroupCreated uses. It self-authorizes on apply
                // (genesis establishes authority; see
                // `ops/namespace/namespace_created.rs`). Previously root creation
                // emitted NO op and the founder lived only in the creator's local
                // GroupMeta, which is exactly the gap #2474 closes.
                if let Some(parent_id) = parent_group_id {
                    // Sealed under the namespace key. Group structure is only
                    // members' business, and the choke point decides — this site
                    // does not know or need to know which variants seal.
                    let create_op = calimero_governance_store::seal_root_op_for_publish(
                        &datastore,
                        namespace_id.to_bytes().into(),
                        RootOp::GroupCreated {
                            // The account this node acts as — the same one written
                            // into the local rows above, so a receiver folds the
                            // creator the rows already name.
                            admin: admin_account,
                            group_id: group_id.to_bytes().into(),
                            parent_id: parent_id.to_bytes().into(),
                            restricted,
                        },
                    )?;
                    match calimero_governance_store::sign_apply_and_publish_namespace_op(
                        &datastore,
                        &node_client,
                        &ack_router,
                        namespace_id.to_bytes().into(),
                        &signer_sk,
                        create_op,
                    )
                    .await
                    {
                        Ok(report) => {
                            report.observe("create_group", "GroupCreated");
                        }
                        Err(e) => {
                            // Subgroup GroupCreated intentionally keeps warn-and-
                            // continue: unlike the namespace-ROOT genesis below, a
                            // subgroup's authoritative state is recoverable by re-
                            // applying the (idempotent) GroupCreated op, and a missing
                            // subgroup op does not strand the namespace founder.
                            tracing::warn!(?e, "failed to publish GroupCreated on namespace DAG");
                        }
                    }
                } else {
                    // Present by construction: the same `parent_group_id` test
                    // that selected this branch is the one that minted it. Named
                    // as an error rather than unwrapped so a future edit that
                    // separates the two fails loudly instead of skipping the
                    // genesis op and leaving a namespace with no founder on the
                    // DAG.
                    let Some(founder_credential) = founder_credential else {
                        eyre::bail!(
                            "internal: namespace-root creation reached the genesis op without \
                             the founder credential it is minted with"
                        );
                    };
                    let genesis_op = NamespaceOp::Root(RootOp::NamespaceCreated {
                        founder: admin_account,
                        account: founder_credential,
                    });
                    match calimero_governance_store::sign_apply_and_publish_namespace_op(
                        &datastore,
                        &node_client,
                        &ack_router,
                        namespace_id.to_bytes().into(),
                        &signer_sk,
                        genesis_op,
                    )
                    .await
                    {
                        Ok(report) => {
                            report.observe("create_group", "NamespaceCreated");
                            // No explicit op-store persist here: the genesis op is
                            // written to the unified op-store ATOMICALLY inside
                            // `sign_apply_and_publish_namespace_op`'s apply (C3 Stage 4,
                            // #2927/#2933), exactly like the GroupCreated branch above.
                        }
                        Err(e) => {
                            // An `Err` here is, by contract, a LOCAL APPLY failure —
                            // never a publish/transport failure (#2474 reviewer batch 5).
                            //
                            // `sign_apply_and_publish_namespace_op` is apply-FIRST and
                            // publish-BEST-EFFORT: it `?`-propagates only the local DAG
                            // mutation (sign/hash/`apply_signed_op`), while EVERY
                            // publish/transport error — including the normal cold-start
                            // `NoPeersSubscribedToTopic` — is caught internally and
                            // downgraded to a `Degraded` `Ok(report)`. So an `Err` here
                            // already means the genesis op did NOT apply to our own
                            // store; there is no no-peers case to special-case (a
                            // namespace created offline / on a single node still gets
                            // `Ok` because apply succeeds and the publish is swallowed).
                            //
                            // RELEASE-SAFE CONTRACT GUARD (#2474 reviewer batch 8):
                            // formerly a `debug_assert!` pinned the above contract — but a
                            // debug_assert is a NO-OP in release. If the apply-first/
                            // publish-best-effort contract ever DRIFTS and a no-peers error
                            // starts surfacing as `Err` in a release build, the rollback
                            // below would WRONGLY fire on a genesis that DID apply locally
                            // (apply-first means a no-peers error implies the local apply
                            // already succeeded), destroying a perfectly-good root. So we
                            // guard at runtime in every build profile: if the `Err` is a
                            // no-peers error, treat it as SUCCESS — skip the rollback and
                            // do NOT return `Err`, because the local apply is known-good.
                            let no_peers =
                                calimero_network_primitives::client::is_no_peers_subscribed_error(
                                    &e,
                                );
                            if no_peers {
                                warn!(
                                    ?group_id,
                                    "no-peers surfaced as Err from apply-first publish \
                                     (contract drift); genesis was applied locally — NOT \
                                     rolling back"
                                );
                                // Falls through to the shared tail rather than
                                // returning: the genesis applied, so what that tail
                                // does - carrying this account's devices into the
                                // namespace - is owed here too.
                            }

                            // FATAL for namespace-ROOT creation (#2474): the genesis op
                            // is what makes the founder authoritative on the DAG. A LOCAL
                            // APPLY failure means the namespace would exist locally with
                            // correct meta but NO genesis on the DAG — and a backfilling
                            // replica would fall back to the broken TOFU seed
                            // (`seed_bootstrap_admin_if_absent`), pinning the wrong admin.
                            // That is exactly the production bug this PR fixes, so a true
                            // apply failure MUST fail the create.
                            //
                            // ROLLBACK (#2474 reviewer batch 3): the local root rows were
                            // already written (the `GroupMetaValue`, the founder Admin
                            // member row, the default caps, and the group encryption key —
                            // all written above in this async
                            // block). Leaving them behind on a genesis-apply
                            // failure would strand an orphaned root: the top-of-handler
                            // "group already exists" guard would then make every retry
                            // with the same group id fail PERMANENTLY (unrecoverable
                            // without store surgery), while the DAG carries no genesis.
                            // calimero-store has no atomic multi-key write, so we undo
                            // each write explicitly, mirroring the writes above, before
                            // returning Err. After this the namespace is cleanly ABSENT
                            // and a retry with the same group id flows through the normal
                            // create path again. Each delete is idempotent; we log (not
                            // propagate) any delete error so a partial rollback can't mask
                            // the original apply failure, and so the most useful error
                            // (the genesis apply failure) is the one surfaced.
                            //
                            // IDENTITY ROW IS DELIBERATELY NOT ROLLED BACK (#2474).
                            // The namespace identity created above by
                            // `get_or_create_namespace_identity` (the keypair backing
                            // `namespace_id` / `admin_identity` / the signing key) is
                            // intentionally left in place here. It is derived
                            // idempotently from the stable `group_id`, so a retry with
                            // the same `group_id` resolves to the SAME identity →
                            // SAME founder/admin → SAME signing key. Deleting it would
                            // risk `get_or_create_namespace_identity` minting a
                            // DIFFERENT identity (hence a different founder) on retry,
                            // which is exactly the divergence #2474 closes. Reusing it
                            // is both safe (it confers no authority on its own —
                            // authority is established only by the genesis op that just
                            // failed) and necessary for a deterministic retry. The only
                            // cost is a harmless dangling identity row if the caller
                            // never retries; that grants nobody anything and is the
                            // correct trade against a non-deterministic founder.
                            //
                            // NAMESPACE DAG HEAD IS DELIBERATELY NOT ROLLED BACK, AND
                            // NEEDS NO ROLLBACK (#2931 reviewer B1). One might fear that
                            // a failed genesis leaves the `NamespaceGovHead` advanced —
                            // so a retry re-signs the genesis with a non-empty
                            // `parent_op_hashes`, which the no-parents genesis check now
                            // treats as a non-genesis NO-OP (#591): the retry would then
                            // NEVER establish the founder, silently wedging the
                            // `group_id`. It cannot happen: the apply is HEAD-ATOMIC by
                            // ordering, not by
                            // transaction. In `NamespaceGovernance::apply_signed_op`
                            // (governance-store) the op-kind apply runs FIRST
                            // (`apply_root_op(op, root)?`, which dispatches the
                            // `NamespaceCreated` genesis), and ONLY on its success does
                            // the function reach `advance_dag_head` + `store_operation`.
                            // A genesis that fails `?`-propagates out of `apply_root_op`
                            // before `advance_dag_head` is ever called, and
                            // `sign_apply_and_publish` only READS the head
                            // (`read_head_record`) to sign against — it never writes it.
                            // So an `Err` here means the head was NEVER advanced: it is
                            // still the empty/absent pre-genesis head, and a retry
                            // re-signs a clean parentless genesis that passes the gate.
                            // There is therefore nothing to undo. (See the
                            // `genesis_apply_failure_leaves_namespace_head_unadvanced`
                            // test in governance-store for the pinned assertion.)
                            //
                            if !no_peers {
                                rollback_local_group_rows(
                                    &datastore,
                                    &group_id,
                                    &admin_account,
                                    Some(key_id),
                                );
                                return Err(eyre::eyre!(
                                    "failed to apply NamespaceCreated genesis on namespace \
                                     DAG; aborting namespace-root creation and rolling back \
                                     local root rows so a retry with the same group id \
                                     succeeds (the genesis must be atomic with root \
                                     creation): {e}"
                                ));
                            }
                        }
                    }

                    // Put the namespace's default capability mask on the DAG.
                    //
                    // The local row written at creation is only this node's copy.
                    // Four other sites seed a namespace's `default_capabilities`
                    // when none is set — the two `NamespaceCreated` arms, the
                    // replica bootstrap, and gossiped group meta — and before
                    // #3969 they all agreed on `CAN_JOIN_OPEN_SUBGROUPS`, so the
                    // disagreement could not arise. Seeding the namespace with
                    // `CAN_AUTHOR_ON_BEHALF` as well made the creator's value
                    // differ from every fallback, and each of those sites is
                    // absence-gated, so a peer that guessed first kept the guess
                    // forever.
                    //
                    // Publishing it as an op is what makes the value REPLICATED
                    // rather than recomputed. `DefaultCapabilitiesSet`'s apply
                    // writes unconditionally (`ops/group/default_capabilities_set.rs`),
                    // so it overwrites a fallback that already landed, and it is
                    // published here — immediately after genesis — so it causally
                    // precedes every admission on the namespace DAG. A peer
                    // applying ops in causal order therefore has the real mask
                    // before any `MemberJoined` copies it into a member row.
                    //
                    // Old peers need no special handling: `DefaultCapabilitiesSet`
                    // long predates this, so they apply it as they always have.
                    // That is the whole reason this is an op rather than a wider
                    // `NamespaceCreated`, which would have been a wire change.
                    if parent_group_id.is_none() {
                        match calimero_governance_store::sign_apply_and_publish(
                            &datastore,
                            &node_client,
                            &ack_router,
                            &group_id,
                            &signer_sk,
                            GroupOp::DefaultCapabilitiesSet {
                                capabilities:
                                    calimero_context_config::MemberCapabilities::from_bits_truncate(
                                        initial_default_capabilities(true),
                                    ),
                            },
                        )
                        .await
                        {
                            Ok(report) => report.observe("create_group", "DefaultCapabilitiesSet"),
                            // Best effort, like `TargetApplicationSet` below: the
                            // local row is already correct, so the creator works
                            // either way. What a failure costs is the REPLICATION,
                            // and the honest handling is to say so rather than to
                            // unwind a namespace that is otherwise fine.
                            Err(e) => tracing::warn!(
                                ?e,
                                "failed to publish DefaultCapabilitiesSet on namespace DAG; \
                                 peers that never receive it fall back to their own seed, \
                                 which does not carry CAN_AUTHOR_ON_BEHALF"
                            ),
                        }
                    }

                    // Put the target on the DAG so a node that only backfills
                    // (a paired device) learns it too. Best effort: it is applied
                    // locally before the publish.
                    if let Some(target_application_id) = effective_application_id {
                        match calimero_governance_store::sign_apply_and_publish(
                            &datastore,
                            &node_client,
                            &ack_router,
                            &group_id,
                            &signer_sk,
                            GroupOp::TargetApplicationSet {
                                bytecode_id,
                                target_application_id,
                                package: target.package.to_string(),
                                version: target.version.to_string(),
                            },
                        )
                        .await
                        {
                            Ok(report) => report.observe("create_group", "TargetApplicationSet"),
                            Err(e) => warn!(
                                ?e,
                                ?group_id,
                                "failed to publish the namespace's target application"
                            ),
                        }
                    }
                }

                // A namespace's name rides its DAG, sealed under the key every member
                // and every paired device holds. A subgroup's stays on this node: sealed
                // under the subgroup's own key it would be unreadable to inherited members.
                if let Some(n) = name {
                    if let Err(e) = calimero_primitives::metadata::validate_metadata_payload(
                        Some(&n),
                        &std::collections::BTreeMap::new(),
                    ) {
                        warn!(?group_id, reason = %e, "ignoring invalid group name on create");
                    } else if parent_group_id.is_some() {
                        MetadataRepository::new(&datastore).set_group(
                            &group_id,
                            &calimero_primitives::metadata::MetadataRecord {
                                name: Some(n),
                                data: std::collections::BTreeMap::new(),
                                updated_at: calimero_governance_store::now_millis(),
                                updated_by: admin_identity,
                            },
                        )?;
                    } else {
                        match calimero_governance_store::sign_apply_and_publish(
                            &datastore,
                            &node_client,
                            &ack_router,
                            &group_id,
                            &signer_sk,
                            GroupOp::GroupMetadataSet {
                                name: Some(n),
                                data: std::collections::BTreeMap::new(),
                            },
                        )
                        .await
                        {
                            Ok(report) => report.observe("create_group", "GroupMetadataSet"),
                            Err(e) => {
                                warn!(?e, ?group_id, "failed to publish the namespace's name")
                            }
                        }
                    }
                }

                // Every device this account already certified belongs in the
                // namespace this creation just gained. The certificate names no
                // namespace, so a fresh endorsement and a key wrap are all that is
                // missing - and the creator is a member at the cut its own genesis
                // established, which is what makes that endorsement admissible.
                //
                // After the genesis, never before: the binding rows a link's endorser
                // is resolved through are written by that genesis, so a link published
                // earlier could not be authorized at all.
                let _carried = calimero_governance_store::bind_known_devices(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &namespace_id,
                    &signer_sk,
                )
                .await;

                // A root this node created is a namespace its account has gained; a
                // subgroup was covered when the account gained its namespace.
                if parent_group_id.is_none() {
                    crate::account_namespace::announce(
                        &datastore,
                        &node_client,
                        &ack_router,
                        namespace_id,
                        crate::account_namespace::AccountNamespaceChange::Gained,
                        "create_group",
                    )
                    .await;
                }

                debug!(
                    ?group_id,
                    ?parent_group_id,
                    %admin_identity,
                    "group created"
                );

                Ok(CreateGroupResponse { group_id })
            }
            .into_actor(self)
            .map(move |res, act, _ctx| {
                // Free the synchronously-reserved id on any failure that the
                // inner rollback didn't already unwind, so the existence guard
                // doesn't wedge the id against a clean retry. Delete is
                // idempotent, so double-deleting after the genesis rollback is
                // harmless; success has replaced the reservation with the final
                // meta, so we leave it in place.
                if res.is_err() {
                    if let Err(err) = MetaRepository::new(&act.datastore).delete(&group_id) {
                        warn!(
                            ?group_id,
                            ?err,
                            "failed to free reserved group id after create error"
                        );
                    }
                }
                res
            }),
        )
    }
}

/// Undo the local rows a group-create wrote before it failed, so a retry with
/// the same id starts clean instead of tripping over half-created state.
///
/// Every delete is idempotent (a no-op on an absent row), so this is safe
/// whether the create failed after the first write or the last, and safe to
/// call more than once (e.g. the outer cleanup map may also drop the meta).
/// `group_key_id` is `None` when the create failed before the encryption key
/// was stored. Errors are logged, not propagated — a partial rollback must not
/// mask the original create failure.
///
/// The namespace identity minted by `get_or_create_namespace_identity` is
/// DELIBERATELY not deleted (see the genesis-failure comment above): it is
/// derived idempotently from the stable group id, so reusing it on retry keeps
/// the founder deterministic (#2474).
/// The capability mask a newly created group seeds every NON-ADMIN member's
/// row from at admission.
///
/// `CAN_JOIN_OPEN_SUBGROUPS` is in both arms and always was: it is what lets a
/// member of this group be inherited into an Open subgroup beneath it, so
/// dropping it strands every later member at the group it joined.
///
/// # Why a namespace also opens to delegated authorship
///
/// `CAN_AUTHOR_ON_BEHALF` is implied by nothing — not membership, not admin,
/// not the subgroup-admit cascade — so a namespace created without it is
/// authorship-closed, and every attested relay admitted to it afterwards needs
/// its own admin-signed op. For a namespace that keeps admitting fleet nodes
/// that is one governance op per admission, published at the moment a node is
/// assigned and the admin is not watching. The grant then reliably arrives
/// late, or not at all, and the symptom is a write refused at
/// `POST .../intents` after the author has already spent a warrant nonce on it.
///
/// Setting it here is the one point that needs no backfill: the mask is COPIED
/// into a member's capability row when that member is admitted
/// (`MembershipRepository::add_member`), so it reaches every member admitted
/// after creation and no member admitted before — and at creation there are
/// none.
///
/// # Namespaces only, and what that costs
///
/// A subgroup keeps the old mask. A grant resolves through the membership
/// anchor, so one on the namespace root already reaches every Open subgroup
/// beneath it — which is where contexts live — and a `Restricted` subgroup is a
/// deliberate membership boundary whose admin should decide its own posture
/// rather than inherit one from a parent it was created to be separate from.
///
/// The cost is stated rather than hidden: this reaches every non-admin member
/// of the namespace, not only attested TEE nodes, so any of them may be named
/// as a warrant's `executor`. What it does NOT confer is the ability to forge
/// one. A delegated write is authorized by the AUTHOR's warrant — signed by
/// their device key, committed to this context, this method and these exact
/// arguments, with its nonce checked unspent and every peer re-verifying both
/// signature layers at the cut — so a holder can only spend a warrant it was
/// deliberately handed. What it gains is the arguments in cleartext and the
/// choice of when, or whether, to publish.
///
/// Admins never receive this at all: `add_member` seeds the default for
/// non-admin roles only, and the capability is not implied by admin. So a
/// namespace creator's own node is the one member this does not cover, and
/// still needs an explicit grant.
///
/// # This is the initial value, not a rule
///
/// It is a plain local row, changed afterwards by `GroupOp::DefaultCapabilitiesSet`
/// (`meroctl group settings set-default-capabilities`). An admin who wants a
/// closed namespace clears the bit; nothing here re-asserts it.
///
/// It is also not recomputed on any other peer. A joiner adopts the namespace's
/// `default_capabilities` verbatim from the join bundle, so two peers cannot
/// disagree about the mask by having been built at different times — which is
/// what would make one peer seed a member row the other would not, and split
/// the answer `account_may_author` gives about the same relay.
fn initial_default_capabilities(is_namespace: bool) -> u32 {
    use calimero_context_config::MemberCapabilities;

    let mut caps = MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS;
    if is_namespace {
        caps |= MemberCapabilities::CAN_AUTHOR_ON_BEHALF;
    }
    caps.bits()
}

fn rollback_local_group_rows(
    datastore: &Store,
    group_id: &ContextGroupId,
    admin_account: &calimero_account::AccountId,
    group_key_id: Option<[u8; 32]>,
) {
    if let Err(re) = MetaRepository::new(datastore).delete(group_id) {
        warn!(?re, ?group_id, "rollback: failed to delete root meta");
    }
    if let Err(re) = MembershipRepository::new(datastore).remove_member(group_id, admin_account) {
        warn!(
            ?re,
            ?group_id,
            "rollback: failed to delete founder member row"
        );
    }
    if let Err(re) = CapabilitiesRepository::new(datastore).delete_default(group_id) {
        warn!(?re, ?group_id, "rollback: failed to delete default caps");
    }
    if let Some(key_id) = group_key_id {
        if let Err(re) = GroupKeyring::new(datastore, *group_id).delete_key_by_id(&key_id) {
            warn!(?re, ?group_id, "rollback: failed to delete group key");
        }
    }
}

fn load_app_meta(
    datastore: &Store,
    application_id: &calimero_primitives::application::ApplicationId,
) -> eyre::Result<ApplicationMetaValue> {
    let handle = datastore.handle();
    let key = calimero_store::key::ApplicationMeta::new(*application_id);
    handle
        .get(&key)?
        .ok_or_else(|| eyre::eyre!("application '{application_id}' not found"))
}

/// A caller-chosen `bytecode_id` must point at locally-present bytecode of the
/// SAME package as the group's application row — otherwise the group would
/// bind to bytecode the node cannot execute, or to another app entirely.
async fn verify_requested_bytecode_id(
    node_client: &calimero_node_primitives::client::NodeClient,
    bytecode_id: &BytecodeId,
    row_blob: [u8; 32],
    expected_package: &str,
) -> eyre::Result<()> {
    let key_bytes = bytecode_id.to_bytes();
    if key_bytes == [0u8; 32] {
        eyre::bail!("bytecode_id must not be zero");
    }
    if key_bytes == row_blob {
        return Ok(()); // the row's own blob is trivially valid
    }
    let blob_id = calimero_primitives::blobs::BlobId::from(key_bytes);
    if !node_client.has_blob(&blob_id)? {
        eyre::bail!(
            "bytecode_id blob '{blob_id}' is not present locally; install that version first"
        );
    }
    let Some(manifest) = node_client.bundle_manifest_for_blob(&blob_id).await? else {
        eyre::bail!("bytecode_id blob '{blob_id}' is not an application bundle");
    };
    if manifest.package != expected_package {
        eyre::bail!(
            "bytecode_id blob '{blob_id}' belongs to package '{}', expected '{expected_package}'",
            manifest.package
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use calimero_store::key::GroupTarget;
    use std::sync::Arc;

    use calimero_context_client::group::CreateGroupRequest;
    use calimero_context_client::local_governance::GroupOp;
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::MemberCapabilities;
    use calimero_governance_store::{
        governance_broadcast, AccountBindingRepository, AccountNamespaceSet,
        CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository,
        MetadataRepository,
    };
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PublicKey;
    use calimero_primitives::metadata::MAX_METADATA_NAME_LEN;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::{key, types, Store};

    use super::{initial_default_capabilities, rollback_local_group_rows};
    use crate::handlers::ensure_account_namespace::ensure_account_namespace;
    use crate::test_support::{actor, certify_device};

    const APP: [u8; 32] = [0xC1; 32];
    const GROUP: [u8; 32] = [0xC2; 32];

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// Seed every local row a group create writes, returning the group key id.
    fn seed_group(store: &Store, group: &ContextGroupId, admin: &PublicKey) -> [u8; 32] {
        // Enrolled so the rows name the account this key resolves to; the
        // rollback assertions below read them back by that account.
        let admin_account = crate::test_support::enrol(store, group, admin);
        MetaRepository::new(store)
            .save(
                group,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: ApplicationId::from([0xCC; 32]),
                        bytecode_id: [0x11; 32],
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 1_700_000_000,
                    admin_identity: admin_account,
                    owner_identity: admin_account,
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("save meta");
        MembershipRepository::new(store)
            .add_member(group, &admin_account, GroupMemberRole::Admin)
            .expect("add admin");
        CapabilitiesRepository::new(store)
            .set_default_capabilities(group, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
            .expect("set default caps");
        let key_id = GroupKeyring::new(store, *group)
            .store_key(&[0x66; 32])
            .expect("store group key");
        key_id
    }

    #[test]
    fn rollback_removes_every_local_group_row() {
        let store = store();
        let group = ContextGroupId::from([0xA0; 32]);
        let admin = PublicKey::from([0x01; 32]);
        let key_id = seed_group(&store, &group, &admin);

        // Sanity: everything is present before the rollback.
        assert!(MetaRepository::new(&store).load(&group).unwrap().is_some());
        assert!(MembershipRepository::new(&store)
            .is_member(&group, &crate::test_support::account_for(&admin))
            .unwrap());
        assert!(CapabilitiesRepository::new(&store)
            .default_capabilities(&group)
            .unwrap()
            .is_some());
        assert!(GroupKeyring::new(&store, group).holds_any_key().unwrap());

        rollback_local_group_rows(
            &store,
            &group,
            &crate::test_support::account_for(&admin),
            Some(key_id),
        );

        // Every row the create wrote is gone; a retry with the same id is clean.
        assert!(
            MetaRepository::new(&store).load(&group).unwrap().is_none(),
            "meta"
        );
        assert!(
            !MembershipRepository::new(&store)
                .is_member(&group, &crate::test_support::account_for(&admin))
                .unwrap(),
            "member"
        );
        assert!(
            CapabilitiesRepository::new(&store)
                .default_capabilities(&group)
                .unwrap()
                .is_none(),
            "caps"
        );
        assert!(
            !GroupKeyring::new(&store, group).holds_any_key().unwrap(),
            "group key"
        );
    }

    #[test]
    fn rollback_is_scoped_to_the_target_group() {
        let store = store();
        let victim = ContextGroupId::from([0xA0; 32]);
        let bystander = ContextGroupId::from([0xB0; 32]);
        let admin = PublicKey::from([0x01; 32]);
        let key_id = seed_group(&store, &victim, &admin);
        let _ = seed_group(&store, &bystander, &admin);

        rollback_local_group_rows(
            &store,
            &victim,
            &crate::test_support::account_for(&admin),
            Some(key_id),
        );

        // The bystander group is untouched — every row type the helper deletes
        // is checked here, so a rollback that used the wrong group id for any
        // one of them is caught.
        assert!(MetaRepository::new(&store)
            .load(&bystander)
            .unwrap()
            .is_some());
        assert!(MembershipRepository::new(&store)
            .is_member(&bystander, &crate::test_support::account_for(&admin))
            .unwrap());
        assert!(CapabilitiesRepository::new(&store)
            .default_capabilities(&bystander)
            .unwrap()
            .is_some());
        assert!(GroupKeyring::new(&store, bystander)
            .holds_any_key()
            .unwrap());
    }

    #[test]
    fn rollback_without_a_key_id_is_a_partial_no_op() {
        // Simulates a create that failed BEFORE storing the group key: the
        // guard for a `None` key id must not panic, and the rows that DO
        // exist are still removed.
        let store = store();
        let group = ContextGroupId::from([0xA0; 32]);
        let admin = PublicKey::from([0x01; 32]);
        MetaRepository::new(&store)
            .save(
                &group,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: ApplicationId::from([0xCC; 32]),
                        bytecode_id: [0x11; 32],
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 1,
                    admin_identity: crate::test_support::account_for(&admin),
                    owner_identity: crate::test_support::account_for(&admin),
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("save meta");

        rollback_local_group_rows(
            &store,
            &group,
            &crate::test_support::account_for(&admin),
            None,
        );

        assert!(MetaRepository::new(&store).load(&group).unwrap().is_none());
    }

    /// The application row a creation resolves its bytecode through.
    fn install_application(store: &Store, application: ApplicationId) {
        let mut handle = store.handle();
        handle
            .put(
                &key::ApplicationMeta::new(application),
                &types::ApplicationMeta::new(
                    key::BlobMeta::new([0x01; 32].into()),
                    1024,
                    "file://test.wasm".into(),
                    vec![].into(),
                    key::BlobMeta::new([0x02; 32].into()),
                    types::PackageInfo {
                        package: "com.test.app".into(),
                        version: "1.0.0".into(),
                        signer_id: "test".into(),
                        state_version: 0,
                    },
                ),
            )
            .expect("install the application");
    }

    /// A namespace this node creates is a namespace it has just gained, and the
    /// devices it already certified belong there. Without the auto-bind the
    /// creation succeeds and the paired device silently never sees the group.
    ///
    /// The devices come from the account namespace's registry, so this holds on
    /// any device of the account, not only the one that did the certifying.
    #[actix::test]
    async fn creating_a_namespace_carries_this_accounts_devices_into_it() {
        let store = store();
        install_application(&store, ApplicationId::from(APP));
        let device = certify_device(&store, 0xC3, &[]);

        let harness = actor::over(store.clone()).await;
        let created = harness
            .manager
            .send(CreateGroupRequest {
                group_id: Some(GROUP.into()),
                bytecode_id: None,
                application_id: Some(ApplicationId::from(APP)),
                name: None,
                parent_group_id: None,
                restricted: false,
            })
            .await
            .expect("the manager answers")
            .expect("the namespace is created");

        assert!(
            AccountBindingRepository::new(&store)
                .is_device_linked(&created.group_id, device)
                .expect("read the bindings"),
            "the device this account already certified has to be bound in the \
             namespace the creation just gained"
        );
    }

    /// The ladder rung is written only by the target op, so it proves the op applied.
    #[actix::test]
    async fn creating_a_namespace_records_its_target_in_governance_state() {
        let store = store();
        install_application(&store, ApplicationId::from(APP));
        calimero_governance_store::NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the account root the founder's credential is minted from");

        let harness = actor::over(store.clone()).await;
        let created = harness
            .manager
            .send(CreateGroupRequest {
                group_id: Some(GROUP.into()),
                bytecode_id: None,
                application_id: Some(ApplicationId::from(APP)),
                name: None,
                parent_group_id: None,
                restricted: false,
            })
            .await
            .expect("the manager answers")
            .expect("the namespace is created");

        let rungs = calimero_governance_store::UpgradeLadderRepository::new(&store)
            .load(&created.group_id)
            .expect("read the ladder");
        let [rung] = rungs.as_slice() else {
            panic!("the creation must record exactly one target rung, got {rungs:?}");
        };
        assert_eq!(
            rung.application_id,
            ApplicationId::from(APP),
            "the rung names the application the namespace was created for"
        );
        assert_eq!(
            rung.bytecode_id, [0x01; 32],
            "the rung names the bytecode blob the application row resolves to"
        );
    }

    /// The group ops this node published for `group_id`, opened with the key it holds.
    fn published_group_ops(
        store: &Store,
        namespace_id: ContextGroupId,
        group_id: ContextGroupId,
    ) -> Vec<GroupOp> {
        calimero_governance_store::NamespaceOpLogService::new(store, namespace_id.to_bytes().into())
            .collect_signed_group_ops_for_group(group_id.to_bytes())
            .expect("read the op log")
            .into_iter()
            .filter_map(|stored| match &stored.signed_op.op {
                calimero_governance_types::NamespaceOp::Group {
                    group_id: op_group_id,
                    key_id,
                    encrypted,
                    ..
                } => calimero_governance_store::decrypt_group_op(
                    store,
                    namespace_id.to_bytes().into(),
                    *op_group_id,
                    key_id.as_bytes(),
                    encrypted,
                )
                .expect("open the op"),
                _ => None,
            })
            .collect()
    }

    /// A name given at creation is the creator's alone unless it rides the DAG:
    /// a follower folds ops, never the creator's local metadata row.
    #[actix::test]
    async fn creating_a_namespace_with_a_name_publishes_the_name_on_the_dag() {
        let store = store();
        install_application(&store, ApplicationId::from(APP));
        calimero_governance_store::NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the account root the founder's credential is minted from");

        let harness = actor::over(store.clone()).await;
        let created = harness
            .manager
            .send(CreateGroupRequest {
                group_id: Some(GROUP.into()),
                bytecode_id: None,
                application_id: Some(ApplicationId::from(APP)),
                name: Some("Test one".to_owned()),
                parent_group_id: None,
                restricted: false,
            })
            .await
            .expect("the manager answers")
            .expect("the namespace is created");

        assert!(
            names_published(&store, created.group_id, created.group_id)
                .contains(&"Test one".to_owned()),
            "the name must be published as a GroupMetadataSet op"
        );
        assert_eq!(
            MetadataRepository::new(&store)
                .group_metadata(&created.group_id)
                .expect("read the metadata")
                .and_then(|record| record.name)
                .as_deref(),
            Some("Test one"),
            "the op's apply must leave the creator holding the name too"
        );
    }

    /// A name op sealed under a Restricted subgroup's key is unreadable to inherited
    /// members and parks every later op of that subgroup, so it must not be published.
    #[actix::test]
    async fn creating_a_subgroup_with_a_name_keeps_the_name_off_the_dag() {
        let store = store();
        install_application(&store, ApplicationId::from(APP));
        calimero_governance_store::NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the account root the founder's credential is minted from");

        let harness = actor::over(store.clone()).await;
        let create = |group: [u8; 32], name: Option<&str>, parent: Option<ContextGroupId>| {
            CreateGroupRequest {
                group_id: Some(group.into()),
                bytecode_id: None,
                application_id: Some(ApplicationId::from(APP)),
                name: name.map(ToOwned::to_owned),
                parent_group_id: parent,
                restricted: true,
            }
        };
        let root = harness
            .manager
            .send(create(GROUP, None, None))
            .await
            .expect("the manager answers")
            .expect("the namespace is created");
        let subgroup = harness
            .manager
            .send(create([0xC5; 32], Some("Test two"), Some(root.group_id)))
            .await
            .expect("the manager answers")
            .expect("the subgroup is created");

        assert!(
            names_published(&store, root.group_id, subgroup.group_id).is_empty(),
            "a subgroup's name must not become an op in the subgroup's log"
        );
        let local = MetadataRepository::new(&store)
            .group_metadata(&subgroup.group_id)
            .expect("read the metadata")
            .expect("the creator keeps the name it was given");
        assert_eq!(local.name.as_deref(), Some("Test two"));
    }

    /// An invalid name is warned-and-ignored, never published and never fatal.
    #[actix::test]
    async fn an_invalid_name_neither_fails_the_create_nor_reaches_the_dag() {
        let store = store();
        install_application(&store, ApplicationId::from(APP));
        calimero_governance_store::NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the account root the founder's credential is minted from");

        let harness = actor::over(store.clone()).await;
        let created = harness
            .manager
            .send(CreateGroupRequest {
                group_id: Some(GROUP.into()),
                bytecode_id: None,
                application_id: Some(ApplicationId::from(APP)),
                name: Some("x".repeat(MAX_METADATA_NAME_LEN + 1)),
                parent_group_id: None,
                restricted: false,
            })
            .await
            .expect("the manager answers")
            .expect("an invalid name must not fail the creation");

        assert!(
            names_published(&store, created.group_id, created.group_id).is_empty(),
            "an invalid name must not be published"
        );
        assert!(
            MetadataRepository::new(&store)
                .group_metadata(&created.group_id)
                .expect("read the metadata")
                .is_none(),
            "and must not be seeded locally either"
        );
    }

    /// The names this node published for `group_id`.
    fn names_published(
        store: &Store,
        namespace_id: ContextGroupId,
        group_id: ContextGroupId,
    ) -> Vec<String> {
        published_group_ops(store, namespace_id, group_id)
            .into_iter()
            .filter_map(|op| match op {
                GroupOp::GroupMetadataSet { name, .. } => name,
                _ => None,
            })
            .collect()
    }

    /// A namespace this node creates is one its account gained, and the other
    /// devices learn it from the DAG, the only thing that reaches them.
    #[actix::test]
    async fn creating_a_namespace_records_it_in_the_account_namespace() {
        let store = store();
        install_application(&store, ApplicationId::from(APP));
        calimero_governance_store::NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the holder's root");

        let harness = actor::over(store.clone()).await;
        let account_namespace = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("the ensure runs")
            .expect("the holder creates its account namespace");
        let created = harness
            .manager
            .send(CreateGroupRequest {
                group_id: Some(GROUP.into()),
                bytecode_id: None,
                application_id: Some(ApplicationId::from(APP)),
                name: None,
                parent_group_id: None,
                restricted: false,
            })
            .await
            .expect("the manager answers")
            .expect("the namespace is created");

        assert_eq!(
            AccountNamespaceSet::new(&store, account_namespace)
                .contains(created.group_id)
                .expect("read the set"),
            Some(Some(ApplicationId::from(APP))),
            "the namespace the creation gained, with the target it was created for"
        );
        assert_eq!(
            AccountNamespaceSet::new(&store, account_namespace)
                .contains(account_namespace)
                .expect("read the set"),
            None,
            "and never the account namespace itself"
        );
    }

    /// The derived id names the account namespace long before one exists, so
    /// announcing into it would write to a DAG that is not there.
    #[actix::test]
    async fn a_gain_before_the_account_namespace_exists_announces_nothing() {
        let store = store();
        // The root and no ensure call: the id resolves, the
        // namespace it names does not exist.
        let devices = calimero_governance_store::NodeDeviceRepository::new(&store);
        let _root = devices.provision_account_root().expect("the holder's root");
        let account_namespace = devices
            .account_namespace()
            .expect("read the account namespace")
            .expect("the holder names one");
        // Targeted, so the announce takes the synchronous path: an app-less one
        // defers instead, and the guard below would never be reached at all.
        install_application(&store, ApplicationId::from(APP));

        let mut harness = actor::over(store.clone()).await;
        let created = harness
            .manager
            .send(CreateGroupRequest {
                group_id: Some(GROUP.into()),
                bytecode_id: None,
                application_id: Some(ApplicationId::from(APP)),
                name: None,
                parent_group_id: None,
                restricted: true,
            })
            .await
            .expect("the manager answers")
            .expect("the namespace is created");

        assert_eq!(
            AccountNamespaceSet::new(&store, account_namespace)
                .contains(created.group_id)
                .expect("read the set"),
            None,
            "nothing may be recorded under a namespace this node does not take part in"
        );
        assert!(
            calimero_governance_store::get_op_head(&store, &account_namespace)
                .expect("read the op head")
                .is_none(),
            "and no op may be applied to the account namespace"
        );
        // The load-bearing one: the two above also hold when a publish is
        // attempted and refused a layer down, so the topic is what pins the guard.
        let topic = governance_broadcast::ns_topic(account_namespace.to_bytes().into()).to_string();
        assert!(
            !harness.broadcast_topics().contains(&topic),
            "no governance broadcast may be attempted on a namespace this node \
             does not take part in"
        );
    }

    /// A leave published while a gain still waits for its target must win, or
    /// the pending gain re-names a namespace the sweep can no longer drop.
    #[actix::test]
    async fn a_gain_still_waiting_is_dropped_when_the_account_leaves_the_namespace() {
        let store = store();
        let devices = calimero_governance_store::NodeDeviceRepository::new(&store);
        let _root = devices.provision_account_root().expect("the holder's root");

        let harness = actor::over(store.clone()).await;
        let account_namespace = ensure_account_namespace(&store, &harness.context_client)
            .await
            .expect("the ensure runs")
            .expect("the holder creates its account namespace");

        let create = |group: [u8; 32]| CreateGroupRequest {
            group_id: Some(group.into()),
            bytecode_id: None,
            application_id: None,
            name: None,
            parent_group_id: None,
            restricted: true,
        };
        // App-less, so both gains defer. The left one is created FIRST, so the
        // control's publish proves the left one's chance to publish has passed.
        let left = harness
            .manager
            .send(create(GROUP))
            .await
            .expect("the manager answers")
            .expect("the namespace is created");
        let control = harness
            .manager
            .send(create([0xC4; 32]))
            .await
            .expect("the manager answers")
            .expect("the namespace is created");

        let account = devices
            .get()
            .expect("read this node's device")
            .expect("it has one")
            .account;
        MembershipRepository::new(&store)
            .remove_member(&left.group_id, &account)
            .expect("the account leaves it while the gain is still waiting");

        let metas = MetaRepository::new(&store);
        for group in [left.group_id, control.group_id] {
            let mut meta = metas.load(&group).expect("read the meta").expect("one row");
            meta.target.application_id = ApplicationId::from(APP);
            metas.save(&group, &meta).expect("the target folds");
        }

        let set = AccountNamespaceSet::new(&store, account_namespace);
        assert!(
            crate::test_support::eventually(|| set
                .contains(control.group_id)
                .expect("read the set")
                == Some(Some(ApplicationId::from(APP))))
            .await,
            "the control gain has to land, and with the target it folded late"
        );
        assert_eq!(
            set.contains(left.group_id).expect("read the set"),
            None,
            "a gain whose namespace the account has left must not be published"
        );
    }

    /// An app-less root keeps the unset target and publishes no target op, the only
    /// writer of a ladder rung.
    #[actix::test]
    async fn an_app_less_root_group_writes_an_unset_target_and_publishes_no_target_op() {
        let store = store();
        calimero_governance_store::NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("the account root the founder's credential is minted from");

        let harness = actor::over(store.clone()).await;
        let created = harness
            .manager
            .send(CreateGroupRequest {
                group_id: Some(GROUP.into()),
                bytecode_id: None,
                application_id: None,
                name: None,
                parent_group_id: None,
                restricted: true,
            })
            .await
            .expect("the manager answers")
            .expect("an app-less root is created");

        let meta = MetaRepository::new(&store)
            .load(&created.group_id)
            .expect("read the meta")
            .expect("the group has a meta row");
        assert_eq!(meta.target, GroupTarget::default());
        assert!(
            calimero_governance_store::UpgradeLadderRepository::new(&store)
                .load(&created.group_id)
                .expect("read the ladder")
                .is_empty(),
            "no target op may be published for a group that targets nothing"
        );
    }

    /// A namespace is created able to host a relay fleet; a subgroup is not.
    ///
    /// The asymmetry is the decision, so it is pinned from both sides: a test
    /// that only checked the namespace would pass just as happily if every
    /// group were opened, which is the widening this is scoped to avoid.
    #[test]
    fn a_namespace_is_created_open_to_delegated_authorship() {
        let namespace = initial_default_capabilities(true);
        let subgroup = initial_default_capabilities(false);

        assert_eq!(
            namespace,
            (MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS
                | MemberCapabilities::CAN_AUTHOR_ON_BEHALF)
                .bits(),
            "a namespace must seed the authorship grant, or every fleet relay \
             admitted to it needs its own admin-signed op"
        );
        assert_eq!(
            subgroup,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
            "a subgroup keeps the old mask: a root grant already reaches every \
             Open subgroup through the membership anchor, and a Restricted one \
             is a boundary whose admin decides its own posture"
        );
    }

    /// The bit that was always there, and must survive the one being added.
    ///
    /// Losing it is silent and late: members still join the group, and only
    /// inheritance into Open subgroups — where contexts actually live — stops
    /// working, for every member admitted from then on.
    #[test]
    fn open_subgroup_inheritance_survives_in_both_arms() {
        for is_namespace in [true, false] {
            let caps =
                MemberCapabilities::from_bits_truncate(initial_default_capabilities(is_namespace));
            assert!(
                caps.contains(MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS),
                "is_namespace={is_namespace}"
            );
        }
    }

    /// Nothing else rides along.
    ///
    /// The mask is copied verbatim into every non-admin member's row, so an
    /// extra bit here is a capability silently granted to every member of every
    /// namespace — the failure mode this test exists to catch early.
    #[test]
    fn no_other_capability_is_seeded() {
        let namespace = initial_default_capabilities(true);

        assert_eq!(
            namespace.count_ones(),
            2,
            "exactly CAN_JOIN_OPEN_SUBGROUPS and CAN_AUTHOR_ON_BEHALF"
        );
        for unexpected in [
            MemberCapabilities::CAN_CREATE_CONTEXT,
            MemberCapabilities::CAN_INVITE_MEMBERS,
            MemberCapabilities::MANAGE_MEMBERS,
            MemberCapabilities::MANAGE_APPLICATION,
            MemberCapabilities::CAN_CREATE_SUBGROUP,
            MemberCapabilities::CAN_DELETE_SUBGROUP,
            MemberCapabilities::CAN_MANAGE_VISIBILITY,
            MemberCapabilities::CAN_MANAGE_METADATA,
        ] {
            assert_eq!(
                namespace & unexpected.bits(),
                0,
                "{unexpected:?} must not be seeded by default"
            );
        }
    }
}
