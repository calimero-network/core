use std::sync::Arc;

use axum::Router;
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use prometheus_client::registry::Registry;
use tracing::{info, warn};

use crate::admin::service::{setup, site};
use crate::auth;
use crate::config::ServerConfig;
use crate::{jsonrpc, metrics, proxy_identity, sse, ws, AdminState};

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
    let proxy_identity = config.use_proxy_identity();
    if proxy_identity {
        info!(
            "proxy auth: taking account-anchored callers' identity from the proxy's \
             X-Auth-Account / X-Auth-Device headers"
        );
    } else if config.proxy_identity {
        warn!("server.proxy_identity is set but ignored: it applies to proxy auth mode only");
    }

    // Resolved once, here, for the same reason `name_this_node` is: this is the
    // first point holding both the configuration and a store. A node that has
    // minted no signing key yet gets `None` and serves no proofs, which is
    // correct rather than a degradation — it is in no namespace, so there is
    // nothing for a caller to be a member of.
    let proof_policy = crate::proof_auth::ProofPolicy::resolve(
        &datastore,
        config
            .admin
            .as_ref()
            .is_some_and(|admin| admin.delegated_access),
    );

    if let Some((path, router)) = jsonrpc::service(
        config,
        ctx_client.clone(),
        node_client.clone(),
        auth_enabled,
    ) {
        app = app.nest(
            &path,
            with_optional_auth(
                router,
                auth_service.clone(),
                proof_policy.clone(),
                proxy_identity,
            ),
        );
        service_count += 1;
    }

    if let Some((path, handler)) = ws::service(
        config,
        node_client.clone(),
        ctx_client.clone(),
        auth_enabled,
    ) {
        app = app.route(
            &path,
            with_optional_auth(
                handler,
                auth_service.clone(),
                proof_policy.clone(),
                proxy_identity,
            ),
        );
        service_count += 1;
    }

    if let Some((path, router)) = sse::service(
        config,
        node_client.clone(),
        ctx_client,
        datastore.clone(),
        auth_enabled,
    ) {
        app = app.nest(
            path,
            with_optional_auth(
                router,
                auth_service.clone(),
                proof_policy.clone(),
                proxy_identity,
            ),
        );
        service_count += 1;
    }

    if let Some((api_path, protected_router, public_router)) = setup(config, shared_state) {
        if let Some((site_path, serve_dir)) = site(config) {
            app = app.nest_service(site_path.as_str(), serve_dir);
        }

        let admin_router =
            with_optional_auth(protected_router, auth_service, proof_policy, proxy_identity)
                .merge(public_router);
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

fn with_optional_auth<R>(
    router: R,
    auth_service: Option<Arc<mero_auth::AuthService>>,
    proof_policy: Option<crate::proof_auth::ProofPolicy>,
    proxy_identity: bool,
) -> R
where
    R: AuthLayerExt,
{
    if let Some(service) = auth_service {
        router.with_auth_guard(service, proof_policy)
    } else if proxy_identity {
        // The protected routes only, the same ones the embedded guard would
        // wrap: the public ones serve callers the proxy never authenticated, so
        // there is no identity of its to read there.
        router.with_proxy_identity()
    } else {
        router
    }
}

trait AuthLayerExt: Sized {
    fn with_auth_guard(
        self,
        service: Arc<mero_auth::AuthService>,
        proof_policy: Option<crate::proof_auth::ProofPolicy>,
    ) -> Self;

    fn with_proxy_identity(self) -> Self;
}

impl AuthLayerExt for Router {
    fn with_auth_guard(
        self,
        service: Arc<mero_auth::AuthService>,
        proof_policy: Option<crate::proof_auth::ProofPolicy>,
    ) -> Self {
        self.layer(auth::guard_layer(service, proof_policy))
    }

    fn with_proxy_identity(self) -> Self {
        self.layer(axum::middleware::from_fn(proxy_identity::inject))
    }
}

impl AuthLayerExt for axum::routing::MethodRouter {
    fn with_auth_guard(
        self,
        service: Arc<mero_auth::AuthService>,
        proof_policy: Option<crate::proof_auth::ProofPolicy>,
    ) -> Self {
        self.layer(auth::guard_layer(service, proof_policy))
    }

    fn with_proxy_identity(self) -> Self {
        self.layer(axum::middleware::from_fn(proxy_identity::inject))
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
