use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{ListNamespacesRequest, NamespaceSummary};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{MetaRepository, MetadataRepository};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;

use crate::ContextManager;
use calimero_governance_store;

/// The namespace rows targeting one of `applications`, or every row when the list
/// is empty.
///
/// The one place an application is resolved to namespaces. Both things that scope
/// by application read `target_application_id` through here - the listing endpoint
/// and pairing's fan-out - so neither can drift from the other's idea of what an
/// application covers.
pub(crate) fn namespace_rows_for_applications(
    store: &Store,
    applications: &[ApplicationId],
) -> eyre::Result<Vec<([u8; 32], GroupMetaValue)>> {
    let entries = MetaRepository::new(store).enumerate_all(0, usize::MAX)?;
    if applications.is_empty() {
        return Ok(entries);
    }
    Ok(entries
        .into_iter()
        .filter(|(_, meta)| applications.contains(&meta.target.application_id))
        .collect())
}

pub(crate) fn collect_namespace_summaries(
    entries: Vec<([u8; 32], GroupMetaValue)>,
    mut node_identity_for_group: impl FnMut(&ContextGroupId) -> Option<(PublicKey, [u8; 32])>,
    mut build_summary: impl FnMut(
        &ContextGroupId,
        &GroupMetaValue,
        &PublicKey,
    ) -> eyre::Result<Option<NamespaceSummary>>,
) -> eyre::Result<Vec<NamespaceSummary>> {
    let mut namespaces = Vec::new();

    for (group_id_bytes, meta) in entries {
        let group_id = ContextGroupId::from(group_id_bytes);

        let Some((node_identity, _)) = node_identity_for_group(&group_id) else {
            continue;
        };

        if let Some(summary) = build_summary(&group_id, &meta, &node_identity)? {
            namespaces.push(summary);
        }
    }

    Ok(namespaces)
}

pub(crate) fn paginate_namespaces(
    namespaces: &[NamespaceSummary],
    offset: usize,
    limit: usize,
) -> Vec<NamespaceSummary> {
    let total = namespaces.len();
    let start = offset.min(total);
    let end = start.saturating_add(limit).min(total);
    namespaces[start..end].to_vec()
}

impl Handler<ListNamespacesRequest> for ContextManager {
    type Result = ActorResponse<Self, <ListNamespacesRequest as Message>::Result>;

    fn handle(
        &mut self,
        ListNamespacesRequest { offset, limit }: ListNamespacesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let entries = namespace_rows_for_applications(&self.datastore, &[])?;
            let namespaces = collect_namespace_summaries(
                entries,
                |group_id| self.node_signing_key(group_id),
                |group_id, meta, node_identity| {
                    MetadataRepository::new(&self.datastore).build_namespace_summary(
                        group_id,
                        meta,
                        node_identity,
                    )
                },
            )?;

            Ok(paginate_namespaces(&namespaces, offset, limit))
        })();

        ActorResponse::reply(result)
    }
}

#[cfg(test)]
mod tests {
    use calimero_store::key::GroupTarget;
    use std::sync::Arc;

    use calimero_context_client::group::NamespaceSummary;
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::Store;

    use super::{
        collect_namespace_summaries, namespace_rows_for_applications, paginate_namespaces,
    };
    use calimero_governance_store::{
        ApplyError, MembershipRepository, MetaRepository, MetadataRepository, NamespaceRepository,
    };

    fn test_summary(namespace_id: [u8; 32]) -> NamespaceSummary {
        NamespaceSummary {
            namespace_id: namespace_id.into(),
            bytecode_id: [0x11; 32].into(),
            target_application_id: ApplicationId::from([0x22; 32]),
            created_at: 1_700_000_000,
            name: None,
            member_count: 1,
            context_count: 2,
            subgroup_count: 3,
        }
    }

    fn test_meta(application_id: [u8; 32]) -> GroupMetaValue {
        GroupMetaValue {
            target: GroupTarget {
                application_id: ApplicationId::from(application_id),
                bytecode_id: [0xAA; 32],
                package: Box::default(),
                version: Box::default(),
            },
            created_at: 1_700_000_000,
            admin_identity: calimero_account::AccountId::from([0x01; 32]),
            owner_identity: calimero_account::AccountId::from([0x01; 32]),
            migration: None,
            auto_join: true,
        }
    }

    #[test]
    fn collect_namespace_summaries_skips_missing_identity() {
        let entries = vec![
            ([0x01; 32], test_meta([0x10; 32])),
            ([0x03; 32], test_meta([0x10; 32])),
        ];

        let result = collect_namespace_summaries(
            entries,
            |group_id| {
                if group_id.to_bytes() == [0x03; 32] {
                    None
                } else {
                    Some((PublicKey::from([0x05; 32]), [0u8; 32]))
                }
            },
            |group_id, _meta, _node_identity| Ok(Some(test_summary(group_id.to_bytes()))),
        )
        .expect("collect should succeed");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].namespace_id, [0x01; 32].into());
    }

    /// The single application-to-namespace resolution, exercised on both of its
    /// answers. Pairing's fan-out narrows through the same function, so a drift
    /// between "which namespaces serve this app" here and there is impossible by
    /// construction rather than by convention.
    #[test]
    fn namespace_rows_for_applications_filters_by_target_application() {
        let app_a = ApplicationId::from([0x10; 32]);
        let app_b = ApplicationId::from([0x20; 32]);
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let meta = MetaRepository::new(&store);
        for (id, app) in [
            ([0x01; 32], app_a),
            ([0x02; 32], app_b),
            ([0x03; 32], app_a),
        ] {
            meta.save(&id.into(), &test_meta(*app))
                .expect("save group meta");
        }

        let mut scoped: Vec<_> = namespace_rows_for_applications(&store, &[app_a])
            .expect("resolve")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        scoped.sort_unstable();
        assert_eq!(scoped, vec![[0x01; 32], [0x03; 32]]);

        assert_eq!(
            namespace_rows_for_applications(&store, &[])
                .expect("resolve")
                .len(),
            3,
            "no application named is every row, not none"
        );
    }

    #[test]
    fn collect_namespace_summaries_propagates_builder_errors() {
        let entries = vec![([0x01; 32], test_meta([0x10; 32]))];

        let err = collect_namespace_summaries(
            entries,
            |_group_id| Some((PublicKey::from([0x05; 32]), [0u8; 32])),
            |_group_id, _meta, _node_identity| Err(ApplyError::UnsupportedOp.into()),
        )
        .expect_err("builder errors should be propagated");

        assert!(matches!(
            err.downcast_ref::<ApplyError>(),
            Some(ApplyError::UnsupportedOp)
        ));
    }

    #[test]
    fn paginate_namespaces_handles_bounds() {
        let namespaces = vec![
            test_summary([0x01; 32]),
            test_summary([0x02; 32]),
            test_summary([0x03; 32]),
        ];

        let page = paginate_namespaces(&namespaces, 1, 10);
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].namespace_id, [0x02; 32].into());
        assert_eq!(page[1].namespace_id, [0x03; 32].into());

        let empty = paginate_namespaces(&namespaces, 10, 10);
        assert!(empty.is_empty());
    }

    #[test]
    fn helper_flow_matches_handler_membership_rules() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let group_id_with_membership =
            calimero_context_config::types::ContextGroupId::from([0x11; 32]);
        let group_id_without_membership =
            calimero_context_config::types::ContextGroupId::from([0x22; 32]);

        let node_identity_sk = calimero_primitives::identity::PrivateKey::from([0x33; 32]);
        let node_identity_pk = node_identity_sk.public_key();

        let meta = GroupMetaValue {
            target: GroupTarget {
                application_id: ApplicationId::from([0x66; 32]),
                bytecode_id: [0x55; 32],
                package: Box::default(),
                version: Box::default(),
            },
            created_at: 1_700_000_000,
            admin_identity: crate::test_support::account_for(&node_identity_pk),
            owner_identity: crate::test_support::account_for(&node_identity_pk),
            migration: None,
            auto_join: true,
        };

        MetaRepository::new(&store)
            .save(&group_id_with_membership, &meta)
            .expect("save group meta with membership");
        MetaRepository::new(&store)
            .save(&group_id_without_membership, &meta)
            .expect("save group meta without membership");

        NamespaceRepository::new(&store)
            .store_identity(
                &group_id_with_membership,
                &node_identity_pk,
                node_identity_sk.as_bytes(),
            )
            .expect("store namespace identity for first namespace");
        NamespaceRepository::new(&store)
            .store_identity(
                &group_id_without_membership,
                &node_identity_pk,
                node_identity_sk.as_bytes(),
            )
            .expect("store namespace identity for second namespace");

        MembershipRepository::new(&store)
            .add_member(
                &group_id_with_membership,
                &crate::test_support::enrol(&store, &group_id_with_membership, &node_identity_pk),
                calimero_primitives::context::GroupMemberRole::Admin,
            )
            .expect("add node identity to first namespace group");

        let entries = namespace_rows_for_applications(&store, &[]).expect("enumerate");
        let namespaces = collect_namespace_summaries(
            entries,
            |group_id| {
                NamespaceRepository::new(&store)
                    .resolve_identity(group_id)
                    .expect("resolve namespace identity")
            },
            |group_id, meta, node_identity| {
                MetadataRepository::new(&store).build_namespace_summary(
                    group_id,
                    meta,
                    node_identity,
                )
            },
        )
        .expect("collect summaries");
        let result = paginate_namespaces(&namespaces, 0, usize::MAX);

        assert_eq!(
            result.len(),
            1,
            "only the namespace with membership is listed"
        );
        assert_eq!(result[0].namespace_id, group_id_with_membership);
    }
}
