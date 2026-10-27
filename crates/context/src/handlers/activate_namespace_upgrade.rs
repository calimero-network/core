use std::future::Future;
use std::time::Duration;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{
    ActivateNamespaceUpgradeRequest, ActivateNamespaceUpgradeResponse, ActivateUpgradeRequest,
    FailedContext,
};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    find_local_signing_identity, get_group_for_context, MetaRepository, NamespaceRepository,
    UpgradeLadderRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{GroupTarget, LadderRung};
use calimero_store::Store;
use eyre::bail;
use tracing::{debug, warn};

use crate::activation::next_rung;
use crate::error::ContextError;
use crate::hlc_fence::loaded_reader_bytecode_id;
use crate::ContextManager;

const ACTIVATE_RETRY_ATTEMPTS: u32 = 3; // tries per context before a retryable failure is listed
const ACTIVATE_RETRY_BASE_DELAY: Duration = Duration::from_millis(200); // doubles after each failed try

/// What an activation switches, read from the store before any context moves.
#[derive(Debug)]
struct ActivationPlan {
    target_application_id: ApplicationId,
    contexts: Vec<(ContextId, PublicKey)>,
}

impl Handler<ActivateNamespaceUpgradeRequest> for ContextManager {
    type Result = ActorResponse<Self, <ActivateNamespaceUpgradeRequest as Message>::Result>;

    fn handle(
        &mut self,
        request: ActivateNamespaceUpgradeRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let plan = match plan_activation(
            &self.datastore,
            |blob| self.node_client.has_blob(blob),
            &request,
        ) {
            Ok(plan) => plan,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let context_client = self.context_client.clone();
        let switch = move |context_id, executor| {
            let context_client = context_client.clone();
            async move {
                context_client
                    .activate_upgrade(ActivateUpgradeRequest {
                        context_id,
                        executor,
                    })
                    .await
            }
        };
        let node_client = self.node_client.clone();
        let drain = move |context_id| {
            let node_client = node_client.clone();
            async move { node_client.drain_absorbed(context_id).await }
        };
        let run = async move {
            Ok(activate_contexts(plan, switch, drain, ACTIVATE_RETRY_BASE_DELAY).await)
        };

        ActorResponse::r#async(run.into_actor(self))
    }
}

/// Switch and drain each planned context, listing the ones that still fail
/// rather than abandoning the rest of the namespace.
async fn activate_contexts<S, SF, D, DF>(
    plan: ActivationPlan,
    switch: S,
    drain: D,
    base_delay: Duration,
) -> ActivateNamespaceUpgradeResponse
where
    S: Fn(ContextId, PublicKey) -> SF,
    SF: Future<Output = eyre::Result<bool>>,
    D: Fn(ContextId) -> DF,
    DF: Future<Output = eyre::Result<usize>>,
{
    let mut response = ActivateNamespaceUpgradeResponse {
        target_application_id: plan.target_application_id,
        switched_contexts: 0,
        replayed_deltas: 0,
        failed_contexts: Vec::new(),
    };
    for (context_id, executor) in plan.contexts {
        let (switched, outcome) =
            activate_context(context_id, executor, &switch, &drain, base_delay).await;
        response.switched_contexts += u32::from(switched);
        match outcome {
            Ok(replayed) => {
                response.replayed_deltas = response
                    .replayed_deltas
                    .saturating_add(u32::try_from(replayed).unwrap_or(u32::MAX));
            }
            Err(failed) => {
                warn!(
                    %context_id,
                    error = %failed.error,
                    retryable = failed.retryable,
                    "context not activated"
                );
                response.failed_contexts.push(failed);
            }
        }
    }
    response
}

/// Switch one context and replay its held deltas, retrying infrastructure failures
/// with a doubling delay. Returns whether it moved, and its replay count or failure.
async fn activate_context<S, SF, D, DF>(
    context_id: ContextId,
    executor: PublicKey,
    switch: &S,
    drain: &D,
    base_delay: Duration,
) -> (bool, Result<usize, FailedContext>)
where
    S: Fn(ContextId, PublicKey) -> SF,
    SF: Future<Output = eyre::Result<bool>>,
    D: Fn(ContextId) -> DF,
    DF: Future<Output = eyre::Result<usize>>,
{
    let mut switched = false;
    let mut delay = base_delay;
    let mut attempt = 1;
    loop {
        let (err, retryable) = match switch(context_id, executor).await {
            Ok(moved) => {
                switched |= moved;
                // Also for a context that was already current: its held deltas
                // may have been left unreplayed by an earlier lazy switch.
                match drain(context_id).await {
                    Ok(replayed) => return (switched, Ok(replayed)),
                    Err(err) => (err.wrap_err("replaying held deltas"), true),
                }
            }
            Err(err) => {
                let retryable = !matches!(
                    err.downcast_ref::<ContextError>(),
                    Some(ContextError::UpgradeSwitchFailed { .. })
                );
                (err, retryable)
            }
        };
        if !retryable || attempt == ACTIVATE_RETRY_ATTEMPTS {
            let failed = FailedContext {
                context_id,
                error: format!("{err:#}"),
                retryable,
            };
            return (switched, Err(failed));
        }
        debug!(%context_id, attempt, %err, "activation failed; retrying");
        tokio::time::sleep(delay).await;
        delay *= 2;
        attempt += 1;
    }
}

/// Resolve the namespace's stored target and the contexts this node can switch.
///
/// Refuses when a release a switch steps through is not installed here, so the
/// switch never fetches or installs one.
fn plan_activation(
    store: &Store,
    has_blob: impl Fn(&BlobId) -> eyre::Result<bool>,
    request: &ActivateNamespaceUpgradeRequest,
) -> eyre::Result<ActivationPlan> {
    let namespace_id = &request.namespace_id;
    let Some(meta) = MetaRepository::new(store).load(namespace_id)? else {
        bail!(ContextError::NamespaceNotFound {
            namespace_id: namespace_id.to_string(),
        });
    };
    let resolved = NamespaceRepository::new(store).resolve(namespace_id)?;
    if resolved != *namespace_id {
        bail!(ContextError::NotANamespaceRoot {
            group_id: namespace_id.to_string(),
            namespace_id: resolved.to_string(),
        });
    }
    let target_application_id = meta.target.application_id;
    check_expected(request, namespace_id, target_application_id)?;
    if let Some(expected) = &request.expected_version {
        if **expected != *meta.target.version {
            bail!(ContextError::UpgradeVersionMismatch {
                group_id: namespace_id.to_string(),
                expected: expected.clone(),
                actual: meta.target.version.to_string(),
            });
        }
    }

    let Coverage { contexts, missing } = coverage(store, &has_blob, namespace_id, |group, app| {
        check_expected(request, group, app)
    })?;
    if let Some(first) = missing.first() {
        bail!(ContextError::BytecodeNotInstalled {
            blob_id: BlobId::from(first.bytecode_id).to_string(),
        });
    }

    Ok(ActivationPlan {
        target_application_id,
        contexts,
    })
}

/// The releases this node must install before a switch of `namespace_id` can move
/// every context it holds: oldest first in ladder order, deduplicated by bytecode,
/// the target included when it is not installed. What `plan_activation` refuses on.
pub(crate) fn missing_releases(
    store: &Store,
    has_blob: impl Fn(&BlobId) -> eyre::Result<bool>,
    namespace_id: &ContextGroupId,
) -> eyre::Result<Vec<LadderRung>> {
    Ok(coverage(store, &has_blob, namespace_id, |_, _| Ok(()))?.missing)
}

/// The contexts a switch covers on this node and the releases it lacks for them.
struct Coverage {
    contexts: Vec<(ContextId, PublicKey)>,
    missing: Vec<LadderRung>,
}

/// Walk the namespace subtree once; `check` vets each group a held context sits in.
fn coverage(
    store: &Store,
    has_blob: &impl Fn(&BlobId) -> eyre::Result<bool>,
    namespace_id: &ContextGroupId,
    check: impl Fn(&ContextGroupId, ApplicationId) -> eyre::Result<()>,
) -> eyre::Result<Coverage> {
    let mut contexts = Vec::new();
    let mut missing = Vec::new();
    for context_id in NamespaceRepository::new(store)
        .collect_subtree_for_cascade(namespace_id)?
        .contexts
    {
        let Some(executor) = find_local_signing_identity(store, &context_id)? else {
            debug!(%context_id, "no local identity in the context; not switching it");
            continue;
        };
        if let Some(group_id) = get_group_for_context(store, &context_id)? {
            if let Some(group) = MetaRepository::new(store).load(&group_id)? {
                check(&group_id, group.target.application_id)?;
                // A zero id names no blob, as in the lazy upgrade's own check.
                if group.target.bytecode_id != [0u8; 32] {
                    let chain = switch_chain(store, &context_id, &group_id, &group.target)?;
                    merge_ordered(&mut missing, not_installed(store, has_blob, &chain)?);
                }
            }
        }
        contexts.push((context_id, executor));
    }
    Ok(Coverage { contexts, missing })
}

/// Add each release of `chain` not yet in `merged`, placing it before the first
/// merged release that follows it in the chain, so ladder order survives the union.
fn merge_ordered(merged: &mut Vec<LadderRung>, chain: Vec<LadderRung>) {
    for (position, rung) in chain.iter().enumerate() {
        if merged.iter().any(|m| m.bytecode_id == rung.bytecode_id) {
            continue;
        }
        let later = &chain[position + 1..];
        let at = merged
            .iter()
            .position(|m| later.iter().any(|l| l.bytecode_id == m.bytecode_id))
            .unwrap_or(merged.len());
        merged.insert(at, rung.clone());
    }
}

/// The releases a switch of `context_id` steps through, oldest first and ending at
/// the group target: the ladder from its bound, walked as the lazy upgrade does.
fn switch_chain(
    store: &Store,
    context_id: &ContextId,
    group_id: &ContextGroupId,
    target: &GroupTarget,
) -> eyre::Result<Vec<LadderRung>> {
    let ladder = UpgradeLadderRepository::new(store).load(group_id)?;
    let mut chain = Vec::new();
    if let Some(mut bound) = loaded_reader_bytecode_id(store, context_id)? {
        for _ in 0..ContextManager::LADDER_HOP_BUDGET {
            let Some(rung) = next_rung(&ladder, bound, target.bytecode_id) else {
                break;
            };
            bound = rung.bytecode_id;
            chain.push(rung);
        }
    }
    chain.push(LadderRung {
        bytecode_id: target.bytecode_id,
        application_id: target.application_id,
        package: target.package.to_string(),
        version: target.version.to_string(),
    });
    Ok(chain)
}

/// The releases of `chain` a switch would fetch or install: a blob that is not local,
/// or a row naming neither it nor a later release (a bundle's versions share one row).
fn not_installed(
    store: &Store,
    has_blob: &impl Fn(&BlobId) -> eyre::Result<bool>,
    chain: &[LadderRung],
) -> eyre::Result<Vec<LadderRung>> {
    let mut missing = Vec::new();
    for (position, rung) in chain.iter().enumerate() {
        let blob_id = BlobId::from(rung.bytecode_id);
        let row = store
            .handle()
            .get(&calimero_store::key::ApplicationMeta::new(
                rung.application_id,
            ))?
            .map(|row| row.bytecode.blob_id());
        let bound_by_row = row.is_some_and(|row| {
            chain[position..].iter().any(|later| {
                later.application_id == rung.application_id
                    && BlobId::from(later.bytecode_id) == row
            })
        });
        if !bound_by_row || !has_blob(&blob_id)? {
            missing.push(rung.clone());
        }
    }
    Ok(missing)
}

/// Refuse when the caller expected another application than `group_id` targets.
fn check_expected(
    request: &ActivateNamespaceUpgradeRequest,
    group_id: &ContextGroupId,
    actual: ApplicationId,
) -> eyre::Result<()> {
    match request.expected_application_id {
        Some(expected) if expected != actual => bail!(ContextError::UpgradeTargetMismatch {
            group_id: group_id.to_string(),
            expected: expected.to_string(),
            actual: actual.to_string(),
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::Arc;

    use calimero_governance_store::register_context_in_group;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;

    use super::*;

    const CREATION_BLOB: [u8; 32] = [0x01; 32];
    const TARGET_BLOB: [u8; 32] = [0x02; 32];

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn app() -> ApplicationId {
        ApplicationId::from([0xAA; 32])
    }

    fn seed_group(store: &Store, id: u8) -> ContextGroupId {
        seed_group_at(store, id, app(), TARGET_BLOB)
    }

    /// A group targeting `bytecode_id` of `application_id`, with that release installed.
    fn seed_group_at(
        store: &Store,
        id: u8,
        application_id: ApplicationId,
        bytecode_id: [u8; 32],
    ) -> ContextGroupId {
        store
            .handle()
            .put(
                &calimero_store::key::ApplicationMeta::new(application_id),
                &calimero_store::types::ApplicationMeta::new(
                    calimero_store::key::BlobMeta::new(bytecode_id.into()),
                    0,
                    Box::default(),
                    Box::default(),
                    calimero_store::key::BlobMeta::new([0u8; 32].into()),
                    calimero_store::types::PackageInfo {
                        package: Box::default(),
                        version: Box::default(),
                        signer_id: Box::default(),
                        state_version: 0,
                    },
                ),
            )
            .unwrap();
        let gid = ContextGroupId::from([id; 32]);
        let admin = PublicKey::from([0x07; 32]);
        MetaRepository::new(store)
            .save(
                &gid,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id,
                        bytecode_id,
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 0,
                    admin_identity: crate::test_support::account_for(&admin),
                    owner_identity: crate::test_support::account_for(&admin),
                    migration: None,
                    auto_join: false,
                },
            )
            .unwrap();
        gid
    }

    fn set_target_version(store: &Store, group: &ContextGroupId, version: &str) {
        let repo = MetaRepository::new(store);
        let mut meta = repo.load(group).unwrap().unwrap();
        meta.target.version = version.into();
        repo.save(group, &meta).unwrap();
    }

    fn seed_context(store: &Store, group: &ContextGroupId, id: u8, local: bool) -> ContextId {
        let context_id = ContextId::from([id; 32]);
        register_context_in_group(store, group, &context_id).unwrap();
        if local {
            store
                .handle()
                .put(
                    &calimero_store::key::ContextIdentity::new(
                        context_id,
                        PublicKey::from([id; 32]),
                    ),
                    &calimero_store::types::ContextIdentity {
                        private_key: Some([0x77; 32]),
                    },
                )
                .unwrap();
        }
        context_id
    }

    fn request(
        namespace: ContextGroupId,
        expected: Option<ApplicationId>,
    ) -> ActivateNamespaceUpgradeRequest {
        ActivateNamespaceUpgradeRequest {
            namespace_id: namespace,
            expected_application_id: expected,
            expected_version: None,
        }
    }

    fn request_version(
        namespace: ContextGroupId,
        version: &str,
    ) -> ActivateNamespaceUpgradeRequest {
        ActivateNamespaceUpgradeRequest {
            expected_version: Some(version.to_owned()),
            ..request(namespace, Some(app()))
        }
    }

    fn installed(_: &BlobId) -> eyre::Result<bool> {
        Ok(true)
    }

    fn refusal(err: &eyre::Report) -> &ContextError {
        err.downcast_ref::<ContextError>().expect("a typed refusal")
    }

    #[test]
    fn an_unknown_namespace_is_refused_as_not_found() {
        let err = plan_activation(
            &store(),
            installed,
            &request(ContextGroupId::from([0x10; 32]), None),
        )
        .unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::NamespaceNotFound { .. }
        ));
    }

    #[test]
    fn an_expected_application_that_differs_from_the_target_is_refused() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let err = plan_activation(
            &store,
            installed,
            &request(ns, Some(ApplicationId::from([0xBB; 32]))),
        )
        .unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::UpgradeTargetMismatch { .. }
        ));
    }

    #[test]
    fn an_expected_version_that_differs_from_the_target_is_refused() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        set_target_version(&store, &ns, "2.0.0");
        let err = plan_activation(&store, installed, &request_version(ns, "3.0.0")).unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::UpgradeVersionMismatch { .. }
        ));
    }

    #[test]
    fn an_expected_version_that_matches_the_target_is_accepted() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        set_target_version(&store, &ns, "2.0.0");
        plan_activation(&store, installed, &request_version(ns, "2.0.0")).unwrap();
    }

    #[test]
    fn a_subgroup_is_refused_as_not_a_namespace_root() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let sub = seed_group(&store, 0x11);
        NamespaceRepository::new(&store).nest(&ns, &sub).unwrap();
        let _ctx = seed_context(&store, &sub, 0x22, true);

        let err = plan_activation(&store, installed, &request(sub, None)).unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::NotANamespaceRoot { .. }
        ));
    }

    #[test]
    fn a_subgroup_targeting_another_application_than_expected_is_refused() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let sub = seed_group_at(&store, 0x11, ApplicationId::from([0xBB; 32]), TARGET_BLOB);
        NamespaceRepository::new(&store).nest(&ns, &sub).unwrap();
        let _root_ctx = seed_context(&store, &ns, 0x21, true);
        let _sub_ctx = seed_context(&store, &sub, 0x22, true);

        let err = plan_activation(&store, installed, &request(ns, Some(app()))).unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::UpgradeTargetMismatch { .. }
        ));
    }

    #[test]
    fn the_stored_target_and_every_local_context_of_the_subtree_are_planned() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let sub = seed_group(&store, 0x11);
        NamespaceRepository::new(&store).nest(&ns, &sub).unwrap();
        let root_ctx = seed_context(&store, &ns, 0x21, true);
        let sub_ctx = seed_context(&store, &sub, 0x22, true);
        let _remote = seed_context(&store, &sub, 0x23, false);

        for expected in [None, Some(app())] {
            let plan = plan_activation(&store, installed, &request(ns, expected)).unwrap();
            assert_eq!(plan.target_application_id, app());
            let mut planned: Vec<_> = plan.contexts.iter().map(|(id, _)| *id).collect();
            planned.sort();
            assert_eq!(planned, vec![root_ctx, sub_ctx]);
        }
    }

    #[test]
    fn a_target_blob_that_is_not_local_is_refused_rather_than_fetched() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let _ctx = seed_context(&store, &ns, 0x21, true);

        let err = plan_activation(&store, |_| Ok(false), &request(ns, None)).unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::BytecodeNotInstalled { .. }
        ));
    }

    /// A namespace whose ladder runs `CREATION_BLOB` -> each of `rungs`, the last
    /// being its target, and one local context still on `CREATION_BLOB`.
    fn seed_ladder(store: &Store, rungs: [[u8; 32]; 3]) -> (ContextGroupId, ContextId) {
        let ns = seed_group_at(store, 0x10, app(), rungs[2]);
        for bytecode_id in rungs {
            UpgradeLadderRepository::new(store)
                .append(
                    &ns,
                    LadderRung {
                        bytecode_id,
                        application_id: app(),
                        package: String::new(),
                        version: String::new(),
                    },
                )
                .unwrap();
        }
        let ctx = seed_context(store, &ns, 0x21, true);
        crate::activation::record_activation(store, &ctx, CREATION_BLOB);
        (ns, ctx)
    }

    #[test]
    fn a_ladder_rung_that_is_not_installed_is_refused_even_when_the_target_is() {
        let store = store();
        let rungs = [[0x03; 32], [0x04; 32], [0x05; 32]];
        let (ns, _ctx) = seed_ladder(&store, rungs);

        let only_target =
            |blob: &BlobId| -> eyre::Result<bool> { Ok(*blob == BlobId::from(rungs[2])) };
        let err = plan_activation(&store, only_target, &request(ns, None)).unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::BytecodeNotInstalled { blob_id }
                if *blob_id == BlobId::from(rungs[0]).to_string()
        ));
    }

    #[test]
    fn a_ladder_of_local_rungs_under_a_row_naming_the_target_is_planned() {
        let store = store();
        let (ns, ctx) = seed_ladder(&store, [[0x03; 32], [0x04; 32], [0x05; 32]]);

        let plan = plan_activation(&store, installed, &request(ns, None)).unwrap();
        let planned: Vec<_> = plan.contexts.iter().map(|(id, _)| *id).collect();
        assert_eq!(planned, vec![ctx]);
    }

    fn only(blobs: &[[u8; 32]]) -> impl Fn(&BlobId) -> eyre::Result<bool> + '_ {
        move |blob| {
            Ok(blobs
                .iter()
                .any(|installed| BlobId::from(*installed) == *blob))
        }
    }

    fn missing_versions(
        store: &Store,
        has_blob: impl Fn(&BlobId) -> eyre::Result<bool>,
        ns: &ContextGroupId,
    ) -> Vec<String> {
        missing_releases(store, has_blob, ns)
            .unwrap()
            .into_iter()
            .map(|rung| rung.version)
            .collect()
    }

    /// A namespace whose ladder is `rungs` (blob, version), the last its target.
    fn seed_versioned(store: &Store, rungs: &[([u8; 32], &str)]) -> ContextGroupId {
        let (target, version) = rungs[rungs.len() - 1];
        let ns = seed_group_at(store, 0x10, app(), target);
        set_target_version(store, &ns, version);
        let repo = MetaRepository::new(store);
        let mut meta = repo.load(&ns).unwrap().unwrap();
        meta.target.package = "pkg".into();
        repo.save(&ns, &meta).unwrap();
        for (bytecode_id, version) in rungs {
            UpgradeLadderRepository::new(store)
                .append(
                    &ns,
                    LadderRung {
                        bytecode_id: *bytecode_id,
                        application_id: app(),
                        package: "pkg".to_owned(),
                        version: (*version).to_owned(),
                    },
                )
                .unwrap();
        }
        ns
    }

    #[test]
    fn nothing_is_missing_when_no_context_is_behind_and_the_target_is_installed() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let ctx = seed_context(&store, &ns, 0x21, true);
        crate::activation::record_activation(&store, &ctx, TARGET_BLOB);

        assert!(missing_versions(&store, only(&[TARGET_BLOB]), &ns).is_empty());
    }

    #[test]
    fn a_target_that_is_not_installed_is_the_only_release_missing() {
        let store = store();
        let ns = seed_versioned(&store, &[(TARGET_BLOB, "2.0.0")]);
        let ctx = seed_context(&store, &ns, 0x21, true);
        crate::activation::record_activation(&store, &ctx, TARGET_BLOB);

        let missing = missing_releases(&store, only(&[]), &ns).unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].bytecode_id, TARGET_BLOB);
        assert_eq!(missing[0].version, "2.0.0");
        assert_eq!(missing[0].package, "pkg");
        assert_eq!(missing[0].application_id, app());
    }

    #[test]
    fn missed_rungs_are_listed_oldest_first_and_end_at_the_target() {
        let store = store();
        let rungs = [([0x03; 32], "1.1.0"), ([0x04; 32], "1.2.0")];
        let ns = seed_versioned(&store, &rungs);
        let ctx = seed_context(&store, &ns, 0x21, true);
        crate::activation::record_activation(&store, &ctx, CREATION_BLOB);

        let versions = missing_versions(&store, only(&[CREATION_BLOB]), &ns);
        assert_eq!(versions, ["1.1.0", "1.2.0"]);
        let versions = missing_versions(&store, only(&[[0x03; 32]]), &ns);
        assert_eq!(versions, ["1.2.0"]);
    }

    #[test]
    fn contexts_at_different_bounds_share_one_deduplicated_list_in_ladder_order() {
        let store = store();
        let rungs = [([0x03; 32], "1.1.0"), ([0x04; 32], "1.2.0")];
        let ns = seed_versioned(&store, &rungs);
        // The context nearer the target is seeded first, so a naive concatenation
        // would list 1.2.0 before the older 1.1.0.
        let near = seed_context(&store, &ns, 0x21, true);
        crate::activation::record_activation(&store, &near, [0x03; 32]);
        let far = seed_context(&store, &ns, 0x22, true);
        crate::activation::record_activation(&store, &far, CREATION_BLOB);

        assert_eq!(missing_versions(&store, only(&[]), &ns), ["1.1.0", "1.2.0"]);
    }

    #[test]
    fn a_subgroup_created_on_an_ancestor_rung_replays_only_what_follows_it() {
        let store = store();
        let rungs = [([0x03; 32], "1.1.0"), ([0x04; 32], "1.2.0")];
        let ns = seed_versioned(&store, &rungs);
        let sub = seed_group_at(&store, 0x11, app(), [0x04; 32]);
        NamespaceRepository::new(&store).nest(&ns, &sub).unwrap();
        for (bytecode_id, version) in rungs {
            UpgradeLadderRepository::new(&store)
                .append(
                    &sub,
                    LadderRung {
                        bytecode_id,
                        application_id: app(),
                        package: "pkg".to_owned(),
                        version: version.to_owned(),
                    },
                )
                .unwrap();
        }
        let ctx = seed_context(&store, &sub, 0x22, true);
        crate::activation::record_activation(&store, &ctx, [0x03; 32]);

        assert_eq!(missing_versions(&store, only(&[]), &ns), ["1.2.0"]);
    }

    #[test]
    fn the_ladder_orders_the_list_not_the_versions() {
        let store = store();
        let rungs = [
            ([0x03; 32], "0.0.72"),
            ([0x04; 32], "0.0.70"),
            ([0x05; 32], "0.0.71"),
        ];
        let ns = seed_versioned(&store, &rungs);
        let ctx = seed_context(&store, &ns, 0x21, true);
        crate::activation::record_activation(&store, &ctx, CREATION_BLOB);

        let versions = missing_versions(&store, only(&[]), &ns);
        assert_eq!(versions, ["0.0.72", "0.0.70", "0.0.71"]);
    }

    #[test]
    fn a_refused_activation_names_the_first_missing_release() {
        let store = store();
        let rungs = [([0x03; 32], "1.1.0"), ([0x04; 32], "1.2.0")];
        let ns = seed_versioned(&store, &rungs);
        let ctx = seed_context(&store, &ns, 0x21, true);
        crate::activation::record_activation(&store, &ctx, CREATION_BLOB);

        let first = missing_releases(&store, only(&[]), &ns).unwrap()[0].bytecode_id;
        let err = plan_activation(&store, only(&[]), &request(ns, None)).unwrap_err();
        assert!(matches!(
            refusal(&err),
            ContextError::BytecodeNotInstalled { blob_id } if *blob_id == BlobId::from(first).to_string()
        ));
    }

    #[test]
    fn a_namespace_with_nothing_local_to_switch_plans_no_contexts() {
        let store = store();
        let ns = seed_group(&store, 0x10);
        let _remote = seed_context(&store, &ns, 0x21, false);

        let plan = plan_activation(&store, |_| Ok(false), &request(ns, None)).unwrap();
        assert!(plan.contexts.is_empty());
    }

    fn plan_of(contexts: &[ContextId]) -> ActivationPlan {
        ActivationPlan {
            target_application_id: app(),
            contexts: contexts
                .iter()
                .map(|id| (*id, PublicKey::from([0x07; 32])))
                .collect(),
        }
    }

    /// What a migration or load failure looks like once the context side typed it.
    fn switch_failed(context_id: ContextId) -> eyre::Report {
        eyre::eyre!("migration trapped").wrap_err(ContextError::UpgradeSwitchFailed {
            context_id: context_id.to_string(),
        })
    }

    fn failures(response: &ActivateNamespaceUpgradeResponse) -> Vec<(ContextId, bool)> {
        response
            .failed_contexts
            .iter()
            .map(|failed| (failed.context_id, failed.retryable))
            .collect()
    }

    #[tokio::test]
    async fn a_context_whose_switch_fails_is_listed_and_the_others_still_switch() {
        let [first, broken, last] = [0x21, 0x22, 0x23].map(|id| ContextId::from([id; 32]));
        let broken_tries = Cell::new(0);
        let switch = |context_id: ContextId, _: PublicKey| {
            let failure = (context_id == broken).then(|| {
                broken_tries.set(broken_tries.get() + 1);
                switch_failed(context_id)
            });
            async move { failure.map_or(Ok(true), Err) }
        };
        let drain = |_: ContextId| async { Ok::<usize, eyre::Report>(2) };

        let response = activate_contexts(
            plan_of(&[first, broken, last]),
            switch,
            drain,
            Duration::ZERO,
        )
        .await;

        assert_eq!(response.switched_contexts, 2);
        assert_eq!(response.replayed_deltas, 4);
        assert_eq!(failures(&response), vec![(broken, false)]);
        assert_eq!(broken_tries.get(), 1, "a failed switch is not retried");
    }

    #[tokio::test]
    async fn an_infrastructure_failure_is_retried_and_not_listed_once_it_succeeds() {
        let ctx = ContextId::from([0x21; 32]);
        let tries = Cell::new(0);
        let switch = |_: ContextId, _: PublicKey| {
            tries.set(tries.get() + 1);
            let first = tries.get() == 1;
            async move {
                if first {
                    Err(eyre::eyre!("context manager mailbox closed"))
                } else {
                    Ok(true)
                }
            }
        };
        let drain = |_: ContextId| async { Ok::<usize, eyre::Report>(0) };

        let response = activate_contexts(plan_of(&[ctx]), switch, drain, Duration::ZERO).await;

        assert_eq!(tries.get(), 2);
        assert_eq!(response.switched_contexts, 1);
        assert!(response.failed_contexts.is_empty());
    }

    #[tokio::test]
    async fn an_infrastructure_failure_outlasting_the_retries_is_listed_as_retryable() {
        let ctx = ContextId::from([0x21; 32]);
        let moved = Cell::new(false);
        let switch = |_: ContextId, _: PublicKey| {
            let first = !moved.replace(true);
            async move { Ok::<bool, eyre::Report>(first) }
        };
        let drains = Cell::new(0);
        let drain = |_: ContextId| {
            drains.set(drains.get() + 1);
            async { Err::<usize, _>(eyre::eyre!("node manager mailbox dropped")) }
        };

        let response = activate_contexts(plan_of(&[ctx]), switch, drain, Duration::ZERO).await;

        assert_eq!(drains.get(), ACTIVATE_RETRY_ATTEMPTS);
        assert_eq!(
            response.switched_contexts, 1,
            "it moved before its replay failed"
        );
        assert_eq!(failures(&response), vec![(ctx, true)]);
    }
}
