//! `GET /admin-api/usage` — per-namespace resource usage on this node.
//!
//! Reports context / member / subgroup counts and a per-column on-disk byte
//! breakdown (state, private_state, delta, governance). Bytes come from
//! `Store::approximate_size`, which for RocksDB samples SST metadata (no
//! scan). Sufficient for plan enforcement in MDMA; not exact.

use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context::group_store::{
    check_group_membership, count_group_members, enumerate_all_groups, enumerate_group_contexts,
    get_parent_group, list_child_groups, resolve_namespace_identity,
};
use calimero_context_config::types::ContextGroupId;
use calimero_server_primitives::admin::{NamespaceUsage, NamespaceUsageBytes, UsageResponse};
use calimero_store::db::Column;
use calimero_store::key::{NAMESPACE_GOV_HEAD_PREFIX, NAMESPACE_GOV_OP_PREFIX, NAMESPACE_IDENTITY_PREFIX};
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{error, warn};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    match collect_usage(&state.store) {
        Ok(namespaces) => ApiResponse {
            payload: UsageResponse { namespaces },
        }
        .into_response(),
        Err(err) => {
            error!(error=?err, "Failed to compute namespace usage");
            parse_api_error(err).into_response()
        }
    }
}

/// Pure helper: walks the store and returns usage rows for every namespace
/// this node is a member of. Extracted so tests can drive it against an
/// in-memory store without spinning up the axum layer.
pub fn collect_usage(store: &Store) -> EyreResult<Vec<NamespaceUsage>> {
    let entries = enumerate_all_groups(store, 0, usize::MAX)?;
    let mut out = Vec::new();

    for (group_id_bytes, _meta) in entries {
        let group_id = ContextGroupId::from(group_id_bytes);

        // Namespaces are groups with no parent.
        if get_parent_group(store, &group_id)?.is_some() {
            continue;
        }

        // Only include namespaces this node actually participates in. We
        // use the stored namespace identity (set on admission) rather than
        // membership alone, matching what `list_namespaces` reports.
        let Some((node_identity, _, _)) = resolve_namespace_identity(store, &group_id)?
        else {
            continue;
        };
        if !check_group_membership(store, &group_id, &node_identity)? {
            continue;
        }

        let context_ids = enumerate_group_contexts(store, &group_id, 0, usize::MAX)
            .unwrap_or_default();
        let context_count = u32::try_from(context_ids.len()).unwrap_or(u32::MAX);
        let member_count =
            u32::try_from(count_group_members(store, &group_id).unwrap_or(0)).unwrap_or(u32::MAX);
        let subgroup_count =
            u32::try_from(list_child_groups(store, &group_id).unwrap_or_default().len())
                .unwrap_or(u32::MAX);

        let mut state: u64 = 0;
        let mut private_state: u64 = 0;
        let mut delta: u64 = 0;
        for context_id in &context_ids {
            let (start, end) = context_prefix_range(context_id.as_ref());
            state = state.saturating_add(probe(store, Column::State, &start, &end));
            private_state =
                private_state.saturating_add(probe(store, Column::PrivateState, &start, &end));
            delta = delta.saturating_add(probe(store, Column::Delta, &start, &end));
        }

        let governance = governance_bytes(store, &group_id_bytes);

        let total = state
            .saturating_add(private_state)
            .saturating_add(delta)
            .saturating_add(governance);

        out.push(NamespaceUsage {
            namespace_id: hex::encode(group_id_bytes),
            context_count,
            member_count,
            subgroup_count,
            bytes: NamespaceUsageBytes {
                state,
                private_state,
                delta,
                governance,
                total,
            },
        });
    }

    Ok(out)
}

/// `[context_id(32)]` .. `[context_id+1(32)]` — prefix scan over every key
/// whose first 32 bytes equal `context_id`. Rolls at 0xFF to `[0x00…]` which
/// is fine: that's the next sibling prefix and RocksDB treats ranges as
/// `[start, end)`.
fn context_prefix_range(context_id: &[u8]) -> ([u8; 32], [u8; 32]) {
    let mut start = [0u8; 32];
    start.copy_from_slice(context_id);
    let mut end = start;
    increment_prefix(&mut end);
    (start, end)
}

/// Bump a fixed-length byte prefix to its lexicographically-next sibling.
/// Returns all-zero if every byte was 0xFF (caller's range becomes the
/// whole column suffix, which is acceptable for usage estimation).
fn increment_prefix(bytes: &mut [u8]) {
    for b in bytes.iter_mut().rev() {
        if *b == 0xFF {
            *b = 0x00;
            continue;
        }
        *b = b.saturating_add(1);
        return;
    }
}

fn probe(store: &Store, col: Column, start: &[u8], end: &[u8]) -> u64 {
    match store.approximate_size(col, start, end) {
        Ok(n) => n,
        Err(err) => {
            warn!(?col, error=?err, "approximate_size probe failed");
            0
        }
    }
}

/// Sum Group-column bytes belonging to a namespace. Three key families
/// already use `namespace_id` as their prefix (`NamespaceIdentity` 0x36,
/// `NamespaceGovOp` 0x38, `NamespaceGovHead` 0x39), so one range probe per
/// prefix is enough. Other Group-column keys are per-group (not
/// per-namespace); attributing them would require iterating child groups
/// and is out of scope for the estimate.
fn governance_bytes(store: &Store, namespace_id: &[u8; 32]) -> u64 {
    let prefixes = [
        NAMESPACE_IDENTITY_PREFIX,
        NAMESPACE_GOV_OP_PREFIX,
        NAMESPACE_GOV_HEAD_PREFIX,
    ];
    let mut total: u64 = 0;
    for prefix in prefixes {
        let mut start = [0u8; 33];
        start[0] = prefix;
        start[1..].copy_from_slice(namespace_id);
        let mut end = start;
        increment_prefix(&mut end[1..]);
        total = total.saturating_add(probe(store, Column::Group, &start, &end));
    }
    total
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context::group_store::{
        add_group_member, save_group_meta, store_namespace_identity,
    };
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{self, GroupMetaValue};
    use calimero_store::slice::Slice;
    use calimero_store::types;
    use calimero_store::Store;

    use super::*;

    #[test]
    fn increment_prefix_bumps_last_byte() {
        let mut b = [0u8; 4];
        b[3] = 0x10;
        increment_prefix(&mut b);
        assert_eq!(b, [0x00, 0x00, 0x00, 0x11]);
    }

    #[test]
    fn increment_prefix_carries_across_ff() {
        let mut b = [0x01, 0xFF, 0xFF];
        increment_prefix(&mut b);
        assert_eq!(b, [0x02, 0x00, 0x00]);
    }

    #[test]
    fn increment_prefix_wraps_at_all_ff() {
        let mut b = [0xFF, 0xFF];
        increment_prefix(&mut b);
        assert_eq!(b, [0x00, 0x00]);
    }

    #[test]
    fn context_prefix_range_is_single_context_scope() {
        let mut cid = [0u8; 32];
        cid[31] = 0x42;
        let (start, end) = context_prefix_range(&cid);
        assert_eq!(start, cid);
        let mut expected_end = cid;
        expected_end[31] = 0x43;
        assert_eq!(end, expected_end);
    }

    fn seed_namespace(
        store: &Store,
        namespace_id: ContextGroupId,
        node_identity_sk: &PrivateKey,
    ) {
        let node_identity_pk = node_identity_sk.public_key();
        let meta = GroupMetaValue {
            app_key: [0xAA; 32],
            target_application_id: ApplicationId::from([0xBB; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            created_at: 1_700_000_000,
            admin_identity: node_identity_pk,
            migration: None,
            auto_join: true,
        };
        save_group_meta(store, &namespace_id, &meta).expect("save meta");
        store_namespace_identity(
            store,
            &namespace_id,
            &node_identity_pk,
            &**node_identity_sk,
            &[0x44; 32],
        )
        .expect("store identity");
        add_group_member(store, &namespace_id, &node_identity_pk, GroupMemberRole::Admin)
            .expect("add member");
    }

    fn write_state(store: &Store, context_id: ContextId, key_tag: u8, value_len: usize) {
        let mut state_key = [0u8; 32];
        state_key[31] = key_tag;
        let k = key::ContextState::new(context_id, state_key);
        let bytes: Slice<'_> = vec![0xCDu8; value_len].into();
        let v = types::ContextState::from(bytes);
        let mut handle = store.handle();
        handle.put(&k, &v).expect("put state");
    }

    fn write_private(store: &Store, context_id: ContextId, key_tag: u8, value_len: usize) {
        let mut state_key = [0u8; 32];
        state_key[31] = key_tag;
        let k = key::ContextPrivateState::new(context_id, state_key);
        let bytes: Slice<'_> = vec![0xEFu8; value_len].into();
        let v = types::ContextPrivateState::from(bytes);
        let mut handle = store.handle();
        handle.put(&k, &v).expect("put private");
    }

    #[test]
    fn collect_usage_skips_non_member_namespaces() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns_a = ContextGroupId::from([0x11; 32]);
        let node_sk = PrivateKey::from([0x33; 32]);
        seed_namespace(&store, ns_a, &node_sk);

        // namespace B: meta exists but no identity + no membership → should be skipped.
        let ns_b = ContextGroupId::from([0x22; 32]);
        let meta = GroupMetaValue {
            app_key: [0xAA; 32],
            target_application_id: ApplicationId::from([0xBB; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            created_at: 1_700_000_000,
            admin_identity: node_sk.public_key(),
            migration: None,
            auto_join: true,
        };
        save_group_meta(&store, &ns_b, &meta).expect("save meta b");

        let rows = collect_usage(&store).expect("collect");
        assert_eq!(rows.len(), 1, "only ns_a with membership reports");
        assert_eq!(rows[0].namespace_id, hex::encode(ns_a.to_bytes()));
    }

    #[test]
    fn collect_usage_reports_per_column_bytes_by_context() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x11; 32]);
        let node_sk = PrivateKey::from([0x33; 32]);
        seed_namespace(&store, ns, &node_sk);

        let ctx_in = ContextId::from([0xAA; 32]);
        let ctx_out = ContextId::from([0xBB; 32]);

        // Register ctx_in in the namespace; ctx_out is written but unattached.
        calimero_context::group_store::register_context_in_group(&store, &ns, &ctx_in)
            .expect("register context");

        write_state(&store, ctx_in, 0x01, 100);
        write_private(&store, ctx_in, 0x02, 200);

        // ctx_out bytes must not be attributed to this namespace.
        write_state(&store, ctx_out, 0x03, 10_000);

        let rows = collect_usage(&store).expect("collect");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.context_count, 1);
        assert!(row.bytes.state > 0, "state bytes reported");
        assert!(row.bytes.private_state > 0, "private_state bytes reported");
        assert!(
            row.bytes.state < 10_000,
            "bytes from ctx_out must not leak in (got {})",
            row.bytes.state
        );
        assert_eq!(
            row.bytes.total,
            row.bytes
                .state
                .saturating_add(row.bytes.private_state)
                .saturating_add(row.bytes.delta)
                .saturating_add(row.bytes.governance),
            "total must sum the four column fields",
        );
    }
}
