use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{ListNamespacesRequest, NamespaceSummary};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::GroupMetaValue;

use crate::group_store;
use crate::ContextManager;

pub(crate) fn collect_namespace_summaries(
    entries: Vec<([u8; 32], GroupMetaValue)>,
    application_filter: Option<ApplicationId>,
    mut node_identity_for_group: impl FnMut(&ContextGroupId) -> Option<(PublicKey, [u8; 32])>,
    mut build_summary: impl FnMut(
        &ContextGroupId,
        &GroupMetaValue,
        &PublicKey,
    ) -> eyre::Result<Option<NamespaceSummary>>,
) -> eyre::Result<Vec<NamespaceSummary>> {
    let mut namespaces = Vec::new();

    for (group_id_bytes, meta) in entries {
        if application_filter
            .as_ref()
            .is_some_and(|application_id| &meta.target_application_id != application_id)
        {
            continue;
        }

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
            let entries = group_store::enumerate_all_groups(&self.datastore, 0, usize::MAX)?;
            let namespaces = collect_namespace_summaries(
                entries,
                None,
                |group_id| self.node_namespace_identity(group_id),
                |group_id, meta, node_identity| {
                    group_store::build_namespace_summary(
                        &self.datastore,
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
    use calimero_context_client::group::NamespaceSummary;
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::UpgradePolicy;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::key::GroupMetaValue;

    use super::{collect_namespace_summaries, paginate_namespaces};
    use crate::group_store::GroupStoreError;

    fn test_summary(namespace_id: [u8; 32]) -> NamespaceSummary {
        NamespaceSummary {
            namespace_id: namespace_id.into(),
            app_key: [0x11; 32].into(),
            target_application_id: ApplicationId::from([0x22; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            created_at: 1_700_000_000,
            alias: None,
            member_count: 1,
            context_count: 2,
            subgroup_count: 3,
        }
    }

    fn test_meta(application_id: [u8; 32]) -> GroupMetaValue {
        GroupMetaValue {
            app_key: [0xAA; 32],
            target_application_id: ApplicationId::from(application_id),
            upgrade_policy: UpgradePolicy::Automatic,
            created_at: 1_700_000_000,
            admin_identity: PublicKey::from([0x01; 32]),
            migration: None,
            auto_join: true,
        }
    }

    #[test]
    fn collect_namespace_summaries_applies_filter_and_skips_missing_identity() {
        let app_a = ApplicationId::from([0x10; 32]);
        let app_b = ApplicationId::from([0x20; 32]);

        let entries = vec![
            ([0x01; 32], test_meta(*app_a)),
            ([0x02; 32], test_meta(*app_b)),
            ([0x03; 32], test_meta(*app_a)),
        ];

        let result = collect_namespace_summaries(
            entries,
            Some(app_a),
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

    #[test]
    fn collect_namespace_summaries_propagates_builder_errors() {
        let entries = vec![([0x01; 32], test_meta([0x10; 32]))];

        let err = collect_namespace_summaries(
            entries,
            None,
            |_group_id| Some((PublicKey::from([0x05; 32]), [0u8; 32])),
            |_group_id, _meta, _node_identity| Err(GroupStoreError::UnsupportedOp.into()),
        )
        .expect_err("builder errors should be propagated");

        assert!(err
            .to_string()
            .contains(&GroupStoreError::UnsupportedOp.to_string()));
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
}
