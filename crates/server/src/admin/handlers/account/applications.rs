use std::collections::BTreeMap;
use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{MetaRepository, NamespaceRepository};
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{
    AccountApplicationApiEntry, AccountApplicationsApiResponse,
};
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::error;

use crate::admin::handlers::account::no_account_error;
use crate::admin::handlers::identity::get_node_identity::node_identity;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// Every application this node's participating namespaces target, deduped and
/// grouped by the namespaces that target each one.
///
/// `None` when this node holds no account, mirroring `GET /admin-api/identity`.
/// A namespace whose metadata has not synced yet contributes nothing rather than
/// erroring - the same "unresolved" treatment `KnownDeviceCert::covers` gives it.
///
/// # Errors
/// Propagates the underlying store scan or read failure.
fn collect(store: &Store) -> EyreResult<Option<Vec<AccountApplicationApiEntry>>> {
    if node_identity(store)?.is_none() {
        return Ok(None);
    }

    let meta = MetaRepository::new(store);
    let mut by_application: BTreeMap<ApplicationId, Vec<ContextGroupId>> = BTreeMap::new();
    for namespace in NamespaceRepository::new(store).participating_namespaces()? {
        if let Some(value) = meta.load(&namespace)? {
            by_application
                .entry(value.target_application_id)
                .or_default()
                .push(namespace);
        }
    }

    Ok(Some(
        by_application
            .into_iter()
            .map(|(application_id, namespaces)| AccountApplicationApiEntry {
                application_id,
                namespaces: namespaces
                    .into_iter()
                    .map(|namespace| hex::encode(namespace.to_bytes()))
                    .collect(),
            })
            .collect(),
    ))
}

/// `GET /admin-api/account/applications`
///
/// The applications this account speaks in, derived from its participating
/// namespaces' target applications - the same mapping `pair-complete`'s scope
/// resolution reads.
pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    match collect(&state.store) {
        Ok(Some(applications)) => ApiResponse {
            payload: AccountApplicationsApiResponse { applications },
        }
        .into_response(),
        Ok(None) => no_account_error().into_response(),
        Err(err) => {
            error!(error = ?err, "Failed to read this account's applications");
            parse_api_error(err).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_governance_store::NodeDeviceRepository;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;

    use super::*;

    const NS_A: [u8; 32] = [0xA1; 32];
    const NS_B: [u8; 32] = [0xB2; 32];
    const NS_C: [u8; 32] = [0xC3; 32];

    fn ns(bytes: [u8; 32]) -> ContextGroupId {
        ContextGroupId::from(bytes)
    }

    fn meta_for(application: ApplicationId) -> GroupMetaValue {
        GroupMetaValue {
            bytecode_id: [0; 32],
            target_application_id: application,
            created_at: 0,
            admin_identity: calimero_account::AccountId::from([0; 32]),
            owner_identity: calimero_account::AccountId::from([0; 32]),
            migration: None,
            auto_join: true,
        }
    }

    /// A store where this node holds an account root, so the account gate
    /// passes.
    fn seeded_account() -> Store {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NodeDeviceRepository::new(&store)
            .ensure_account_root()
            .expect("mint a root");
        store
    }

    #[test]
    fn applications_dedupe_across_namespaces_of_the_same_app() {
        let store = seeded_account();
        let namespaces = NamespaceRepository::new(&store);
        let meta = MetaRepository::new(&store);
        let app = ApplicationId::from([0x77; 32]);

        for namespace in [NS_A, NS_B] {
            namespaces.note_participation(&ns(namespace)).expect("join");
            meta.save(&ns(namespace), &meta_for(app))
                .expect("save meta");
        }

        let applications = collect(&store).expect("collect").expect("has account");

        assert_eq!(applications.len(), 1);
        assert_eq!(applications[0].application_id, app);
        let mut want = vec![hex::encode(NS_A), hex::encode(NS_B)];
        want.sort();
        let mut got = applications[0].namespaces.clone();
        got.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn distinct_applications_are_reported_separately() {
        let store = seeded_account();
        let namespaces = NamespaceRepository::new(&store);
        let meta = MetaRepository::new(&store);
        let app_one = ApplicationId::from([0x11; 32]);
        let app_two = ApplicationId::from([0x22; 32]);

        namespaces.note_participation(&ns(NS_A)).expect("join A");
        meta.save(&ns(NS_A), &meta_for(app_one))
            .expect("save meta A");
        namespaces.note_participation(&ns(NS_C)).expect("join C");
        meta.save(&ns(NS_C), &meta_for(app_two))
            .expect("save meta C");

        let applications = collect(&store).expect("collect").expect("has account");

        let mut got: Vec<ApplicationId> = applications
            .iter()
            .map(|entry| entry.application_id)
            .collect();
        got.sort();
        let mut want = vec![app_one, app_two];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn a_node_holding_no_account_reports_none() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));

        assert!(collect(&store).expect("read").is_none());
    }
}
