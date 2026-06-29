use std::sync::Arc;

use axum::Router;
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use prometheus_client::registry::Registry;

use crate::admin::service::{setup, site};
use crate::auth;
use crate::config::ServerConfig;
use crate::{jsonrpc, metrics, sse, ws, AdminState};

#[derive(Debug)]
pub(crate) struct MountedService {
    pub router: Router,
    pub added_count: usize,
}

pub(crate) fn mount_runtime_services(
    app: Router,
    config: &ServerConfig,
    deps: RuntimeServiceDeps,
) -> MountedService {
    let RuntimeServiceDeps {
        auth_service,
        ctx_client,
        node_client,
        datastore,
        shared_state,
        prom_registry,
    } = deps;
    let mut app = app;
    let mut service_count = 0usize;
    let auth_enabled = auth_service.is_some();

    if let Some((path, router)) = jsonrpc::service(
        config,
        ctx_client.clone(),
        node_client.clone(),
        auth_enabled,
    ) {
        app = app.nest(&path, with_optional_auth(router, auth_service.clone()));
        service_count += 1;
    }

    if let Some((path, handler)) =
        ws::service(config, node_client.clone(), ctx_client, auth_enabled)
    {
        app = app.route(&path, with_optional_auth(handler, auth_service.clone()));
        service_count += 1;
    }

    if let Some((path, router)) =
        sse::service(config, node_client.clone(), datastore.clone(), auth_enabled)
    {
        app = app.nest(path, with_optional_auth(router, auth_service.clone()));
        service_count += 1;
    }

    if let Some((api_path, protected_router, public_router)) = setup(config, shared_state) {
        if let Some((site_path, serve_dir)) = site(config) {
            app = app.nest_service(site_path.as_str(), serve_dir);
        }

        let admin_router = with_optional_auth(protected_router, auth_service).merge(public_router);
        app = app.nest(&api_path, admin_router);
        service_count += 1;
    }

    if let Some((path, router)) = metrics::service(config, prom_registry) {
        app = app.nest(path, router);
        service_count += 1;
    }

    MountedService {
        router: app,
        added_count: service_count,
    }
}

fn with_optional_auth<R>(router: R, auth_service: Option<Arc<mero_auth::AuthService>>) -> R
where
    R: AuthLayerExt,
{
    if let Some(service) = auth_service {
        router.with_auth_guard(service)
    } else {
        router
    }
}

trait AuthLayerExt: Sized {
    fn with_auth_guard(self, service: Arc<mero_auth::AuthService>) -> Self;
}

impl AuthLayerExt for Router {
    fn with_auth_guard(self, service: Arc<mero_auth::AuthService>) -> Self {
        self.layer(auth::guard_layer(service))
    }
}

impl AuthLayerExt for axum::routing::MethodRouter {
    fn with_auth_guard(self, service: Arc<mero_auth::AuthService>) -> Self {
        self.layer(auth::guard_layer(service))
    }
}

/// Runtime dependencies threaded into [`mount_runtime_services`].
pub(crate) struct RuntimeServiceDeps {
    pub auth_service: Option<Arc<mero_auth::AuthService>>,
    pub ctx_client: ContextClient,
    pub node_client: NodeClient,
    pub datastore: Store,
    pub shared_state: Arc<AdminState>,
    pub prom_registry: Registry,
}
