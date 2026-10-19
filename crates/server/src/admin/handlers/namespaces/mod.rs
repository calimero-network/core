use calimero_context_client::group::TargetRelease;

pub mod admit_join;
pub mod change_admin;
pub mod create_group_in_namespace;
pub mod create_namespace;
pub mod delete_namespace;
pub mod get_namespace;
pub mod invite_namespace;
pub mod join_namespace;
pub mod leave_namespace;
pub mod link_device;
pub mod list;
pub mod list_for_application;
pub mod list_namespace_groups;
pub mod revoke_device;
pub mod revoked_devices;

/// Per-namespace `appVersion`: the bundle-manifest version of the
/// namespace's `bytecode_id` blob. `None` when unresolvable (zero/legacy key,
/// raw-wasm app, blob not retained locally) — display-only, never an error.
pub(crate) async fn namespace_app_version(
    node_client: &calimero_node_primitives::client::NodeClient,
    bytecode_id: [u8; 32],
) -> Option<String> {
    if bytecode_id == [0u8; 32] {
        return None;
    }
    node_client
        .blob_app_version(&calimero_primitives::blobs::BlobId::from(bytecode_id))
        .await
}

/// `appVersion` and `appPackage` for a namespace: the blob's manifest version
/// when this node holds it, else the version the group's target names, so a
/// member that has not installed the release yet still reports it.
pub(crate) fn with_target_release(
    blob_version: Option<String>,
    target: Option<TargetRelease>,
) -> (Option<String>, Option<String>) {
    let package = target.as_ref().map(|release| release.package.clone());
    (
        blob_version.or(target.map(|release| release.version)),
        package,
    )
}

/// The founder and salt `namespace_id` was derived from, as recorded when this
/// node applied the namespace's genesis. Display-only like `appVersion`: a
/// failed read is logged and reported as absent rather than failing the request.
pub(crate) fn namespace_founding(
    store: &calimero_store::Store,
    namespace_id: &calimero_context_config::types::ContextGroupId,
) -> Option<calimero_server_primitives::admin::NamespaceFoundingApi> {
    match calimero_governance_store::NamespaceFoundingRepository::new(store).get(namespace_id) {
        Ok(found) => found.map(|(founder, salt)| founding_api(&founder, &salt)),
        Err(err) => {
            tracing::warn!(
                ?err,
                ?namespace_id,
                "could not read the namespace founding record"
            );
            None
        }
    }
}

/// The group ops this node holds unapplied in `namespace_id`, or `None` when
/// there are none (or they cannot be read: the field is diagnostic, so a read
/// fault is logged rather than failing the request).
pub(crate) fn namespace_held_ops(
    store: &calimero_store::Store,
    namespace_id: &calimero_context_config::types::ContextGroupId,
) -> Option<calimero_server_primitives::admin::NamespaceHeldOpsApi> {
    let held =
        calimero_governance_store::held_ops::HeldOps::new(store, namespace_id.to_bytes().into());
    match held.read_live() {
        Ok(record) if record.ops.is_empty() && record.untracked == 0 => None,
        Ok(record) => Some(calimero_server_primitives::admin::NamespaceHeldOpsApi {
            ops: record
                .ops
                .iter()
                .map(|op| calimero_server_primitives::admin::NamespaceHeldOpApi {
                    delta_id: hex::encode(op.delta_id),
                    group_id: hex::encode(op.group_id),
                })
                .collect(),
            untracked: record.untracked,
        }),
        Err(err) => {
            tracing::warn!(?err, ?namespace_id, "could not read the held-ops record");
            None
        }
    }
}

pub(crate) fn founding_api(
    founder: &calimero_account::AccountId,
    salt: &[u8; 32],
) -> calimero_server_primitives::admin::NamespaceFoundingApi {
    calimero_server_primitives::admin::NamespaceFoundingApi {
        founder_account_id: hex::encode(founder.as_bytes()),
        salt: hex::encode(salt),
    }
}

#[cfg(test)]
mod tests {
    use super::{with_target_release, TargetRelease};

    fn release() -> Option<TargetRelease> {
        Some(TargetRelease {
            package: "com.acme.app".to_owned(),
            version: "2.0.0".to_owned(),
        })
    }

    #[test]
    fn an_uninstalled_release_reports_the_targets_version() {
        assert_eq!(
            with_target_release(None, release()),
            (Some("2.0.0".to_owned()), Some("com.acme.app".to_owned()))
        );
    }

    #[test]
    fn a_held_blob_keeps_its_own_version() {
        assert_eq!(
            with_target_release(Some("1.9.0".to_owned()), release()),
            (Some("1.9.0".to_owned()), Some("com.acme.app".to_owned()))
        );
    }

    #[test]
    fn a_group_with_no_named_release_reports_nothing() {
        assert_eq!(with_target_release(None, None), (None, None));
    }
}
