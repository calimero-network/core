use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    AccountNamespaceSet, MetaRepository, NamespaceRepository, NodeDeviceRepository,
};
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

/// Every application this ACCOUNT speaks in, grouped by the namespaces
/// targeting each one, whether or not this device takes part in them.
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

    let account_namespace = NodeDeviceRepository::new(store).account_namespace()?;
    let participating: BTreeSet<ContextGroupId> = NamespaceRepository::new(store)
        .participating_namespaces()?
        .into_iter()
        .collect();

    let mut by_application: BTreeMap<ApplicationId, AccountApplicationApiEntry> = BTreeMap::new();
    for (namespace, application, coords) in
        namespace_targets(store, account_namespace, &participating)?
    {
        let entry =
            by_application
                .entry(application)
                .or_insert_with(|| AccountApplicationApiEntry {
                    application_id: application,
                    namespaces: Vec::new(),
                    package: None,
                    version: None,
                    followed: false,
                });
        entry.namespaces.push(hex::encode(namespace.to_bytes()));
        // Participation outlives a narrowing, so it alone does not say "followed".
        entry.followed |= participating.contains(&namespace)
            && calimero_context::account_follow::node_reaches(store, &namespace)?;
        if let Some((package, version)) = coords {
            entry.package = Some(package);
            entry.version = Some(version);
        }
    }

    Ok(Some(by_application.into_values().collect()))
}

/// Every namespace an application is known for, with the coordinates the account
/// namespace recorded for it.
///
/// The account's own set comes first and wins: it names namespaces this device
/// takes no part in, which is the whole reason the route exists. Local
/// participation then fills in anything the set has not learned - a namespace
/// gained before the account recorded one, or a node holding no account
/// namespace at all.
fn namespace_targets(
    store: &Store,
    account_namespace: Option<ContextGroupId>,
    participating: &BTreeSet<ContextGroupId>,
) -> EyreResult<Vec<(ContextGroupId, ApplicationId, Option<(String, String)>)>> {
    let mut seen = BTreeSet::new();
    let mut targets = Vec::new();

    if let Some(account_namespace) = account_namespace {
        let set = AccountNamespaceSet::new(store, account_namespace);
        for (namespace, gained_application) in set.namespaces()? {
            let named = set.target(namespace)?;
            // The named target is the later word: a `TargetApplicationSet` after
            // the gain refreshes it, and the gain is never restated.
            let Some(application) = named
                .as_ref()
                .map(|target| target.application)
                .or(gained_application)
            else {
                continue;
            };
            let _ = seen.insert(namespace);
            targets.push((
                namespace,
                application,
                named.map(|target| (target.package, target.version)),
            ));
        }
    }

    let meta = MetaRepository::new(store);
    for namespace in participating {
        if Some(*namespace) == account_namespace || seen.contains(namespace) {
            continue;
        }
        if let Some(value) = meta.load(namespace)? {
            targets.push((*namespace, value.target.application_id, None));
        }
    }
    Ok(targets)
}

/// `GET /admin-api/account/applications`
///
/// The applications this account speaks in, from the namespace set its own
/// namespace replicates to every device - so a device whose scope leaves an
/// application out still learns of it, and where to install it from.
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
    use calimero_store::key::{GroupMetaValue, GroupTarget};

    use super::*;

    const NS_A: [u8; 32] = [0xA1; 32];
    const NS_B: [u8; 32] = [0xB2; 32];
    const NS_C: [u8; 32] = [0xC3; 32];

    fn ns(bytes: [u8; 32]) -> ContextGroupId {
        ContextGroupId::from(bytes)
    }

    fn meta_for(application: ApplicationId) -> GroupMetaValue {
        GroupMetaValue {
            target: GroupTarget {
                application_id: application,
                ..Default::default()
            },
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
            .provision_account_root()
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

    /// The account namespace targets nothing and is not a project, so it never
    /// contributes an application.
    #[test]
    fn the_account_namespace_contributes_no_application() {
        let store = seeded_account();
        let devices = NodeDeviceRepository::new(&store);
        let namespaces = NamespaceRepository::new(&store);
        let meta = MetaRepository::new(&store);
        let app = ApplicationId::from([0x77; 32]);
        let account_namespace = devices
            .account_root()
            .expect("read the root")
            .expect("seeded_account mints one")
            .account_namespace();

        namespaces.note_participation(&ns(NS_A)).expect("join A");
        meta.save(&ns(NS_A), &meta_for(app)).expect("save meta A");
        namespaces
            .note_participation(&account_namespace)
            .expect("follow the account namespace");
        meta.save(
            &account_namespace,
            &GroupMetaValue {
                target: GroupTarget::default(),
                ..meta_for(app)
            },
        )
        .expect("save the account namespace meta");
        devices
            .store_account_namespace(&account_namespace)
            .expect("record it");

        let applications = collect(&store).expect("collect").expect("has account");

        assert_eq!(applications.len(), 1);
        assert_eq!(applications[0].application_id, app);
    }

    /// A narrowed device keeps its participation row for a later widening, so the
    /// application it lost stays listed but must stop reading as followed.
    #[test]
    fn a_narrowed_device_stops_following_the_application_it_lost() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let namespaces = NamespaceRepository::new(&store);
        let meta = MetaRepository::new(&store);
        let app_kept = ApplicationId::from([0x11; 32]);
        let app_lost = ApplicationId::from([0x22; 32]);
        for (namespace, application) in [(NS_A, app_kept), (NS_B, app_lost)] {
            namespaces.note_participation(&ns(namespace)).expect("join");
            meta.save(&ns(namespace), &meta_for(application))
                .expect("save meta");
        }
        let account_namespace = ns(NS_C);
        let (device, root_sk) = calimero_context::test_support::paired_device_scoped_to(
            &store,
            &account_namespace,
            &[],
        );

        let followed = |store: &Store| -> Vec<ApplicationId> {
            let mut applications: Vec<_> = collect(store)
                .expect("collect")
                .expect("has account")
                .iter()
                .filter(|entry| entry.followed)
                .map(|entry| entry.application_id)
                .collect();
            applications.sort();
            applications
        };
        let mut both = vec![app_kept, app_lost];
        both.sort();
        assert_eq!(followed(&store), both);

        calimero_context::test_support::rescope_paired_device(
            &store,
            &account_namespace,
            device,
            &root_sk,
            &[app_kept],
            1,
        );

        assert_eq!(
            followed(&store),
            vec![app_kept],
            "an application this device was narrowed out of is no longer followed"
        );
    }

    /// The reason the route exists: an application on the account that this
    /// device's scope leaves out, listed with the coordinates to install it from
    /// and marked as not followed - beside one the device does take part in.
    #[test]
    fn an_application_outside_this_devices_scope_is_listed_unfollowed_with_coordinates() {
        let store = seeded_account();
        let devices = NodeDeviceRepository::new(&store);
        let account_namespace = devices
            .account_root()
            .expect("read the root")
            .expect("seeded_account mints one")
            .account_namespace();
        devices
            .store_account_namespace(&account_namespace)
            .expect("record it");
        let followed_app = ApplicationId::from([0x11; 32]);
        let outside_app = ApplicationId::from([0x22; 32]);

        // NS_A: gained, targeted, and this device takes part in it.
        NamespaceRepository::new(&store)
            .note_participation(&ns(NS_A))
            .expect("join A");
        MetaRepository::new(&store)
            .save(&ns(NS_A), &meta_for(followed_app))
            .expect("save meta A");
        let set = AccountNamespaceSet::new(&store, account_namespace);
        set.record(ns(NS_A), Some(followed_app)).expect("gain A");
        set.name_target(ns(NS_A), followed_app, "com.acme.a", "1.0.0")
            .expect("name A's target");
        // NS_B: gained by another device of the account, out of this one's scope,
        // so there is no participation row and no metadata here at all.
        set.record(ns(NS_B), Some(outside_app)).expect("gain B");
        set.name_target(ns(NS_B), outside_app, "com.acme.b", "2.0.0")
            .expect("name B's target");

        let applications = collect(&store).expect("collect").expect("has account");

        let outside = applications
            .iter()
            .find(|entry| entry.application_id == outside_app)
            .expect("the out-of-scope application must be listed");
        assert_eq!(outside.namespaces, vec![hex::encode(NS_B)]);
        assert_eq!(outside.package.as_deref(), Some("com.acme.b"));
        assert_eq!(outside.version.as_deref(), Some("2.0.0"));
        assert!(
            !outside.followed,
            "this device takes part in no namespace of that application"
        );

        let followed = applications
            .iter()
            .find(|entry| entry.application_id == followed_app)
            .expect("the in-scope application must still be listed");
        assert_eq!(followed.namespaces, vec![hex::encode(NS_A)]);
        assert_eq!(followed.package.as_deref(), Some("com.acme.a"));
        assert!(followed.followed, "this device takes part in NS_A");
    }

    /// A namespace this node takes part in that the account's set has never
    /// named keeps the pre-set behaviour: listed, followed, no coordinates.
    #[test]
    fn a_namespace_the_account_set_never_named_is_still_listed() {
        let store = seeded_account();
        let app = ApplicationId::from([0x77; 32]);

        NamespaceRepository::new(&store)
            .note_participation(&ns(NS_A))
            .expect("join A");
        MetaRepository::new(&store)
            .save(&ns(NS_A), &meta_for(app))
            .expect("save meta A");

        let applications = collect(&store).expect("collect").expect("has account");

        assert_eq!(applications.len(), 1);
        assert_eq!(applications[0].application_id, app);
        assert!(applications[0].followed);
        assert_eq!(applications[0].package, None);
    }

    #[test]
    fn a_node_holding_no_account_reports_none() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));

        assert!(collect(&store).expect("read").is_none());
    }
}
