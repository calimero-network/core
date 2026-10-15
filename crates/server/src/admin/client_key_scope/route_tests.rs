//! A bound client key against the real admin router and auth guard, on a node
//! holding two applications' namespaces, contexts and blobs.

use std::sync::Arc;

use actix::{Actor, Context as ActixContext, Handler};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use calimero_context_client::client::ContextClient;
use calimero_context_client::group::{GroupContextEntry, NamespaceSummary};
use calimero_context_client::messages::ContextMessage;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{ContextTreeService, MetaRepository};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_store::db::InMemoryDB;
use calimero_store::key::{ApplicationMeta, ContextMeta as ContextMetaKey, GroupMetaValue};
use calimero_store::types::ContextMeta;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use mero_auth::auth::bindings::application_binding;
use mero_auth::auth::token::TokenManager;
use mero_auth::config::JwtConfig;
use mero_auth::secrets::SecretManager;
use mero_auth::storage::{Key, KeyManager, MemoryStorage, Storage};
use mero_auth::AuthService;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt as _;

use crate::admin::service::{setup, AdminConfig};
use crate::config::ServerConfig;
use crate::service_mounts::with_optional_auth;
use crate::{AdminState, NodeReadiness};

const APP_TOKEN: &[&str] = &[
    "context:create",
    "context:list",
    "context:execute",
    "context:subscribe",
    "application:list",
    "namespace",
    "group",
    "blob",
    "context:alias",
];

struct Tenant {
    app: ApplicationId,
    namespace: ContextGroupId,
    context: ContextId,
    blob: BlobId,
}

fn tenant(byte: u8) -> Tenant {
    Tenant {
        app: ApplicationId::from([byte; 32]),
        namespace: ContextGroupId::from([byte; 32]),
        context: ContextId::from([byte; 32]),
        blob: BlobId::from([0; 32]),
    }
}

/// Answers the actor reads the routes under test make, from the store.
struct StoreBackedContexts(Store);

impl Actor for StoreBackedContexts {
    type Context = ActixContext<Self>;
}

impl Handler<ContextMessage> for StoreBackedContexts {
    type Result = ();

    fn handle(&mut self, msg: ContextMessage, _ctx: &mut Self::Context) {
        match msg {
            ContextMessage::ListNamespaces { outcome, .. } => {
                let _ignored = outcome.send(Ok(namespaces(&self.0, None)));
            }
            ContextMessage::ListNamespacesForApplication { request, outcome } => {
                let _ignored = outcome.send(Ok(namespaces(&self.0, Some(&request.application_id))));
            }
            ContextMessage::ListGroupContexts { request, outcome } => {
                let _ignored = outcome.send(
                    ContextTreeService::new(&self.0, request.group_id)
                        .enumerate_contexts(request.offset, request.limit)
                        .map(|ids| {
                            ids.into_iter()
                                .map(|context_id| GroupContextEntry {
                                    context_id,
                                    name: None,
                                })
                                .collect()
                        }),
                );
            }
            ContextMessage::GetGroupForContext { request, outcome } => {
                let _ignored = outcome.send(calimero_governance_store::get_group_for_context(
                    &self.0,
                    &request.context_id,
                ));
            }
            _ => {}
        }
    }
}

fn namespaces(store: &Store, app: Option<&ApplicationId>) -> Vec<NamespaceSummary> {
    MetaRepository::new(store)
        .enumerate_all(0, usize::MAX)
        .expect("metas")
        .into_iter()
        .filter(|(_, meta)| app.is_none_or(|a| meta.target.application_id == *a))
        .map(|(id, meta)| NamespaceSummary {
            namespace_id: ContextGroupId::from(id),
            bytecode_id: meta.target.bytecode_id.into(),
            target_application_id: meta.target.application_id,
            created_at: meta.created_at,
            name: None,
            member_count: 0,
            context_count: 0,
            subgroup_count: 0,
        })
        .collect()
}

struct Node {
    router: Router,
    proxied: Router,
    proxied_with_identity: Router,
    store: Store,
    node_client: calimero_node_primitives::client::NodeClient,
    keys: KeyManager,
    tokens: TokenManager,
    mine: Tenant,
    theirs: Tenant,
    _blob_dir: TempDir,
}

impl Node {
    async fn new() -> Self {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        calimero_governance_store::NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("this node's account root");

        let (event_sender, _rx) = tokio::sync::broadcast::channel(16);
        let (node_client, blob_dir) = crate::test_support::test_node_client(
            &store,
            crate::test_support::stub_node_manager(vec![]),
            event_sender,
        )
        .await;
        let contexts = LazyRecipient::<ContextMessage>::new();
        let handle = contexts.clone();
        let backing = store.clone();
        let _addr = StoreBackedContexts::create(move |ctx| {
            assert!(handle.init(ctx), "context manager recipient init");
            StoreBackedContexts(backing)
        });
        let ctx_client = ContextClient::new(store.clone(), node_client.clone(), contexts);

        let mut mine = tenant(0xa1);
        let mut theirs = tenant(0xb1);
        for t in [&mut mine, &mut theirs] {
            seed_tenant(&store, &node_client, t).await;
        }

        let state = Arc::new(AdminState::new(
            store.clone(),
            ctx_client,
            node_client.clone(),
            Arc::new(NodeReadiness::new()),
            [0; 32],
            #[cfg(feature = "mock-attestation")]
            false,
        ));
        let config = ServerConfig::new(
            vec![],
            libp2p::identity::Keypair::generate_ed25519(),
            Some(AdminConfig::new(true, false)),
            None,
            None,
            None,
        );
        let (path, protected, _public) = setup(&config, state).expect("admin api enabled");

        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let secrets = Arc::new(SecretManager::new(Arc::clone(&storage)));
        secrets.initialize().await.unwrap();
        let tokens = TokenManager::new(
            JwtConfig {
                issuer: "test".to_owned(),
                access_token_expiry: 3600,
                refresh_token_expiry: 86400,
                client_access_token_expiry: mero_auth::config::default_client_access_token_expiry(),
                client_refresh_token_expiry: mero_auth::config::default_client_refresh_token_expiry(
                ),
                node_host: None,
            },
            Arc::clone(&storage),
            secrets,
        );
        let keys = KeyManager::new(storage);
        let root = Key::new_root_key_with_permissions(
            "owner".to_owned(),
            "user_password".to_owned(),
            vec!["admin".to_owned()],
            None,
        );
        keys.set_key("root", &root).await.unwrap();
        let auth = Arc::new(AuthService::new(Vec::new(), tokens.clone()));
        let proxied = Router::new().nest(
            &path,
            with_optional_auth(protected.clone(), None, None, false),
        );
        let proxied_with_identity = Router::new().nest(
            &path,
            with_optional_auth(protected.clone(), None, None, true),
        );
        let router = Router::new().nest(
            &path,
            with_optional_auth(protected, Some(auth), None, false),
        );

        Self {
            router,
            proxied,
            proxied_with_identity,
            store,
            node_client,
            keys,
            tokens,
            mine,
            theirs,
            _blob_dir: blob_dir,
        }
    }

    /// A token for a client key holding `permissions`, stored as the mint stores it.
    async fn client_token(&self, permissions: Vec<String>) -> String {
        let key = Key::new_client_key(
            "root".to_owned(),
            "app".to_owned(),
            permissions.clone(),
            None,
        );
        let id = uuid::Uuid::new_v4().to_string();
        self.keys.set_key(&id, &key).await.unwrap();
        self.tokens
            .generate_token_pair(id, permissions, None, None)
            .await
            .unwrap()
            .0
    }

    async fn bound_to_mine(&self) -> String {
        let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
        permissions.push(application_binding(&self.mine.app.to_string()));
        self.client_token(permissions).await
    }

    async fn bound_to_mine_with(&self, extra: &str) -> String {
        let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
        permissions.push(extra.to_owned());
        permissions.push(application_binding(&self.mine.app.to_string()));
        self.client_token(permissions).await
    }

    async fn unbound(&self) -> String {
        self.client_token(APP_TOKEN.iter().map(|p| (*p).to_owned()).collect())
            .await
    }

    async fn call(
        &self,
        token: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(format!("/admin-api{path}"))
            .header("Authorization", format!("Bearer {token}"));
        Self::send(&self.router, request, body).await
    }

    /// A request as forward-auth hands it on: the token's permissions in `X-Auth-Permissions`.
    async fn call_proxied(
        &self,
        permissions: &[String],
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(format!("/admin-api{path}"))
            .header("X-Auth-Permissions", permissions.join(","));
        Self::send(&self.proxied, request, body).await
    }

    async fn send(
        router: &Router,
        mut request: axum::http::request::Builder,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let body = match body {
            Some(json) => {
                request = request.header("Content-Type", "application/json");
                Body::from(json.to_string())
            }
            None => Body::empty(),
        };
        let response = router
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

async fn seed_tenant(
    store: &Store,
    node_client: &calimero_node_primitives::client::NodeClient,
    t: &mut Tenant,
) {
    let account = calimero_primitives::identity::AccountId::from([0x33; 32]);
    MetaRepository::new(store)
        .save(
            &t.namespace,
            &GroupMetaValue {
                target: calimero_store::key::GroupTarget {
                    application_id: t.app,
                    bytecode_id: [0xaa; 32],
                    package: Box::default(),
                    version: Box::default(),
                },
                created_at: 1_700_000_000,
                admin_identity: account,
                owner_identity: account,
                migration: None,
                auto_join: true,
            },
        )
        .expect("meta");
    ContextTreeService::new(store, t.namespace)
        .register_context(&t.context)
        .expect("register");
    store
        .handle()
        .put(
            &ContextMetaKey::new(t.context),
            &ContextMeta::new(ApplicationMeta::new(t.app), [0; 32], vec![], None),
        )
        .expect("context meta");

    let bytes = t.context.as_ref().to_vec();
    let (blob, _) = node_client
        .add_blob(futures_util::io::Cursor::new(bytes), None, None)
        .await
        .expect("blob");
    node_client
        .record_blob_context(&blob, &t.context)
        .expect("associate");
    t.blob = blob;
}

fn ids(body: &Value, list: &str, field: &str) -> Vec<String> {
    body["data"][list]
        .as_array()
        .or_else(|| body["data"].as_array())
        .or_else(|| body[list].as_array())
        .unwrap_or_else(|| panic!("a list in {body}"))
        .iter()
        .map(|row| row[field].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[actix::test]
async fn a_bound_key_lists_only_its_own_applications_contexts() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;

    let (status, body) = node.call(&token, Method::GET, "/contexts", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids(&body, "contexts", "id"),
        vec![node.mine.context.to_string()]
    );
}

#[actix::test]
async fn a_bound_key_lists_only_its_own_applications_namespaces() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;

    let (status, body) = node.call(&token, Method::GET, "/namespaces", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed = ids(&body, "namespaces", "namespaceId");
    assert_eq!(
        listed,
        vec![hex::encode(node.mine.namespace.to_bytes())],
        "{body}"
    );
}

#[actix::test]
async fn a_bound_key_is_refused_another_applications_listings() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let theirs = node.theirs.app;

    for path in [
        format!("/contexts/for-application/{theirs}"),
        format!("/contexts/with-executors/for-application/{theirs}"),
        format!("/namespaces/for-application/{theirs}"),
        format!("/applications/{theirs}"),
    ] {
        let (status, body) = node.call(&token, Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
    }
}

#[actix::test]
async fn a_bound_key_cannot_manage_another_applications_namespace() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let ns = hex::encode(node.theirs.namespace.to_bytes());

    for (method, path) in [
        (Method::DELETE, format!("/namespaces/{ns}")),
        (Method::POST, format!("/namespaces/{ns}/invite")),
        (Method::POST, format!("/namespaces/{ns}/groups")),
        (Method::POST, format!("/groups/{ns}/members")),
        (Method::DELETE, format!("/groups/{ns}")),
    ] {
        let (status, body) = node
            .call(&token, method.clone(), &path, Some(json!({})))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
    }
}

#[actix::test]
async fn a_bound_key_cannot_create_in_another_application() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let (theirs, ns) = (
        node.theirs.app,
        hex::encode(node.theirs.namespace.to_bytes()),
    );
    let mine = hex::encode(node.mine.namespace.to_bytes());

    for (path, body) in [
        (
            "/contexts",
            json!({ "applicationId": theirs, "groupId": ns, "initializationParams": [] }),
        ),
        (
            "/contexts",
            json!({ "applicationId": theirs, "groupId": mine, "initializationParams": [] }),
        ),
        ("/namespaces", json!({ "applicationId": theirs })),
        (
            "/groups",
            json!({ "applicationId": theirs, "parentGroupId": ns }),
        ),
        ("/groups", json!({ "applicationId": theirs })),
    ] {
        let (status, response) = node
            .call(&token, Method::POST, path, Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path} {body}: {response}");
    }
}

#[actix::test]
async fn a_bound_key_reaches_only_its_own_applications_blobs() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let (mine, theirs) = (&node.mine, &node.theirs);

    let (status, _) = node.call(&token, Method::GET, "/blobs", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "listing every blob");

    let (status, _) = node
        .call(
            &token,
            Method::GET,
            &format!("/blobs/{}?context_id={}", theirs.blob, theirs.context),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "naming their context");
    let (status, _) = node
        .call(
            &token,
            Method::GET,
            &format!("/blobs/{}?context_id={}", theirs.blob, mine.context),
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "their blob through my context"
    );
    let (status, _) = node
        .call(
            &token,
            Method::GET,
            &format!("/blobs/{}", theirs.blob),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no context named");
    let (status, _) = node
        .call(
            &token,
            Method::DELETE,
            &format!("/blobs/{}", theirs.blob),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "deleting their blob");

    let (status, _) = node
        .call(
            &token,
            Method::GET,
            &format!("/blobs/{}?context_id={}", mine.blob, mine.context),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "reading my blob");
    let (status, _) = node
        .call(
            &token,
            Method::DELETE,
            &format!("/blobs/{}", mine.blob),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "deleting my blob");
}

#[actix::test]
async fn a_bound_key_still_reaches_its_own_application() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let (app, ctx) = (node.mine.app, node.mine.context);

    for path in [
        format!("/contexts/{ctx}"),
        format!("/contexts/for-application/{app}"),
        format!("/namespaces/for-application/{app}"),
        format!("/contexts/{ctx}/group"),
    ] {
        let (status, body) = node.call(&token, Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
}

/// A group this node does not hold has no signed target to check the binding
/// against: the invitation's application is an unsigned hint.
#[actix::test]
async fn a_bound_key_cannot_join_a_group_this_node_does_not_hold() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let unknown = ContextGroupId::from([0xc1; 32]);
    let unknown_hex = hex::encode(unknown.to_bytes());
    let mut invitation = calimero_context::test_support::unverified_invitation(&unknown);
    invitation.application_id = Some(*node.mine.app.as_ref());
    let invite = json!({ "invitation": invitation });

    for path in [
        format!("/namespaces/{unknown_hex}/join"),
        "/groups/join".to_owned(),
    ] {
        let (status, body) = node
            .call(&token, Method::POST, &path, Some(invite.clone()))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
    }
    let mut held = calimero_context::test_support::unverified_invitation(&node.mine.namespace);
    held.application_id = Some(*node.mine.app.as_ref());
    let (status, body) = node
        .call(
            &token,
            Method::POST,
            "/groups/join",
            Some(json!({ "invitation": held })),
        )
        .await;
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "a held group of mine: {body}"
    );
}

#[actix::test]
async fn an_unbound_key_keeps_the_node_wide_view() {
    let node = Node::new().await;
    let token = node.unbound().await;

    let (_, body) = node.call(&token, Method::GET, "/contexts", None).await;
    let mut listed = ids(&body, "contexts", "id");
    listed.sort();
    let mut all = vec![
        node.mine.context.to_string(),
        node.theirs.context.to_string(),
    ];
    all.sort();
    assert_eq!(listed, all);

    let (_, body) = node.call(&token, Method::GET, "/namespaces", None).await;
    assert_eq!(ids(&body, "namespaces", "namespaceId").len(), 2, "{body}");
}

#[actix::test]
async fn a_context_bound_key_lists_only_its_context() {
    let node = Node::new().await;
    let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
    permissions.push(format!("context[{},identity]", node.mine.context));
    let token = node.client_token(permissions).await;

    let (status, body) = node.call(&token, Method::GET, "/contexts", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids(&body, "contexts", "id"),
        vec![node.mine.context.to_string()]
    );
}

#[actix::test]
async fn a_context_bound_key_lists_only_its_context_in_its_group() {
    let node = Node::new().await;
    let sibling = ContextId::from([0xa0; 32]);
    ContextTreeService::new(&node.store, node.mine.namespace)
        .register_context(&sibling)
        .expect("register");
    let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
    permissions.push(format!("context[{},identity]", node.mine.context));
    let token = node.client_token(permissions).await;

    let ns = hex::encode(node.mine.namespace.to_bytes());
    let (status, body) = node
        .call(
            &token,
            Method::GET,
            &format!("/groups/{ns}/contexts?limit=1"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids(&body, "contexts", "contextId"),
        vec![node.mine.context.to_string()]
    );
}

#[actix::test]
async fn a_bound_key_sees_and_changes_only_its_own_applications_aliases() {
    let node = Node::new().await;
    let owner = node.unbound().await;
    for (alias, context) in [("mine", node.mine.context), ("theirs", node.theirs.context)] {
        let (status, body) = node
            .call(
                &owner,
                Method::POST,
                "/alias/create/context",
                Some(json!({ "alias": alias, "contextId": context })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let token = node.bound_to_mine().await;

    let (_, body) = node
        .call(&token, Method::GET, "/alias/list/context", None)
        .await;
    let listed: Vec<&String> = body["data"].as_object().expect("a map").keys().collect();
    assert_eq!(listed, vec!["mine"], "{body}");

    let (_, body) = node
        .call(&token, Method::POST, "/alias/lookup/context/theirs", None)
        .await;
    assert_eq!(body["data"]["value"], Value::Null, "{body}");

    let (status, _) = node
        .call(&token, Method::POST, "/alias/delete/context/theirs", None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "deleting their alias");
    let (status, _) = node
        .call(
            &token,
            Method::POST,
            "/alias/create/context",
            Some(json!({ "alias": "x", "contextId": node.theirs.context })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "aliasing their context");
    let (status, _) = node
        .call(&token, Method::POST, "/alias/delete/context/mine", None)
        .await;
    assert_eq!(status, StatusCode::OK, "deleting my alias");
}

#[actix::test]
async fn a_bound_key_cannot_retarget_its_namespace_or_install() {
    let node = Node::new().await;
    let token = node.bound_to_mine_with("application:install").await;
    let ns = hex::encode(node.mine.namespace.to_bytes());

    let (status, body) = node
        .call(
            &token,
            Method::POST,
            &format!("/groups/{ns}/upgrade"),
            Some(json!({ "targetApplicationId": node.theirs.app })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = node
        .call(
            &token,
            Method::POST,
            "/install-application",
            Some(json!({ "package": "p", "version": "1.0.0" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[actix::test]
async fn a_bound_key_joins_only_into_its_own_application() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let invite = |group: ContextGroupId, app: &ApplicationId| {
        let mut invitation = calimero_context::test_support::unverified_invitation(&group);
        invitation.application_id = Some(*app.as_ref());
        json!({ "invitation": invitation })
    };
    let unknown = ContextGroupId::from([0xc1; 32]);
    let unknown_hex = hex::encode(unknown.to_bytes());

    let (status, body) = node
        .call(
            &token,
            Method::POST,
            "/groups/join",
            Some(invite(node.theirs.namespace, &node.theirs.app)),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "their held namespace: {body}"
    );
    let (status, body) = node
        .call(
            &token,
            Method::POST,
            "/groups/join",
            Some(invite(node.theirs.namespace, &node.mine.app)),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a held namespace relabelled: {body}"
    );
    let (status, body) = node
        .call(
            &token,
            Method::POST,
            &format!("/namespaces/{unknown_hex}/join"),
            Some(invite(unknown, &node.theirs.app)),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an unheld namespace of theirs: {body}"
    );
}

#[actix::test]
async fn a_context_bound_key_lists_only_its_context_for_its_application() {
    let node = Node::new().await;
    let sibling = ContextId::from([0xa0; 32]);
    ContextTreeService::new(&node.store, node.mine.namespace)
        .register_context(&sibling)
        .expect("register");
    node.store
        .handle()
        .put(
            &ContextMetaKey::new(sibling),
            &ContextMeta::new(ApplicationMeta::new(node.mine.app), [0; 32], vec![], None),
        )
        .expect("context meta");
    let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
    permissions.push(format!("context[{},identity]", node.mine.context));
    let token = node.client_token(permissions).await;

    let app = node.mine.app;
    for path in [
        format!("/contexts/for-application/{app}"),
        format!("/contexts/with-executors/for-application/{app}"),
    ] {
        let (status, body) = node.call(&token, Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        let listed = ids(&body, "contexts", "id");
        assert_eq!(
            listed,
            vec![node.mine.context.to_string()],
            "{path}: {body}"
        );
    }
}

#[actix::test]
async fn a_bound_key_releases_only_its_own_reference_to_a_shared_blob() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let shared = node.mine.context.as_ref().to_vec();
    let (blob, _) = node
        .node_client
        .add_blob(futures_util::io::Cursor::new(shared), None, None)
        .await
        .expect("blob");
    assert_eq!(blob, node.mine.blob, "the same bytes are the same blob");
    node.node_client
        .record_blob_context(&blob, &node.theirs.context)
        .expect("associate");

    let path = format!("/blobs/{blob}");
    let (status, _) = node.call(&token, Method::DELETE, &path, None).await;
    assert_eq!(status, StatusCode::OK, "releasing mine");
    let (status, _) = node.call(&token, Method::DELETE, &path, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "releasing theirs");
    assert!(
        node.node_client.has_blob(&blob).unwrap(),
        "their copy survives"
    );
}

fn seed_application(node: &Node, app: ApplicationId, bytecode: BlobId) {
    use calimero_store::types::{ApplicationMeta as ApplicationRow, PackageInfo};
    node.store
        .handle()
        .put(
            &ApplicationMeta::new(app),
            &ApplicationRow::new(
                calimero_store::key::BlobMeta::new(bytecode),
                1,
                "calimero://test".into(),
                Box::default(),
                calimero_store::key::BlobMeta::new([0; 32].into()),
                PackageInfo {
                    package: "".into(),
                    version: "".into(),
                    signer_id: "".into(),
                    state_version: 0,
                },
            ),
        )
        .expect("application row");
}

#[actix::test]
async fn a_bound_key_lists_only_its_own_application() {
    let node = Node::new().await;
    seed_application(&node, node.mine.app, node.mine.blob);
    seed_application(&node, node.theirs.app, node.theirs.blob);
    let token = node.bound_to_mine().await;

    let (status, body) = node.call(&token, Method::GET, "/applications", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed = ids(&body, "apps", "id");
    assert_eq!(listed, vec![node.mine.app.to_string()], "{body}");
}

#[actix::test]
async fn a_refused_blob_delete_keeps_the_keys_access() {
    let node = Node::new().await;
    seed_application(&node, node.mine.app, node.mine.blob);
    let token = node.bound_to_mine().await;
    let (mine, ctx) = (node.mine.blob, node.mine.context);

    let (status, _) = node
        .call(&token, Method::DELETE, &format!("/blobs/{mine}"), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an application artifact");
    let (status, _) = node
        .call(
            &token,
            Method::GET,
            &format!("/blobs/{mine}?context_id={ctx}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "still mine to read");
}

#[actix::test]
async fn a_delete_that_releases_nothing_keeps_the_association() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let absent = BlobId::from([0x77; 32]);
    node.node_client
        .record_blob_context(&absent, &node.mine.context)
        .expect("associate");

    let (status, _) = node
        .call(&token, Method::DELETE, &format!("/blobs/{absent}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(node
        .node_client
        .is_blob_in_context(&absent, &node.mine.context)
        .unwrap());
}

#[actix::test]
async fn another_applications_artifact_is_missing_to_a_bound_key() {
    let node = Node::new().await;
    seed_application(&node, node.theirs.app, node.theirs.blob);
    let token = node.bound_to_mine().await;

    let (status, _) = node
        .call(
            &token,
            Method::DELETE,
            &format!("/blobs/{}", node.theirs.blob),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[actix::test]
async fn a_bound_key_behind_forward_auth_is_held_to_its_binding() {
    let node = Node::new().await;
    let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
    permissions.push(application_binding(&node.mine.app.to_string()));

    let (status, body) = node
        .call_proxied(&permissions, Method::GET, "/contexts", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids(&body, "contexts", "id"),
        vec![node.mine.context.to_string()]
    );
    let theirs = node.theirs.context;
    let (status, body) = node
        .call_proxied(
            &permissions,
            Method::GET,
            &format!("/contexts/{theirs}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// A subgroup of `mine`'s namespace targeting its application, holding one context.
fn seed_subgroup(node: &Node) -> (ContextGroupId, ContextId) {
    let sub = ContextGroupId::from([0xa2; 32]);
    let context = ContextId::from([0xa3; 32]);
    let meta = MetaRepository::new(&node.store)
        .load(&node.mine.namespace)
        .expect("load")
        .expect("namespace meta");
    MetaRepository::new(&node.store)
        .save(&sub, &meta)
        .expect("meta");
    calimero_governance_store::NamespaceRepository::new(&node.store)
        .nest(&node.mine.namespace, &sub)
        .expect("nest");
    ContextTreeService::new(&node.store, sub)
        .register_context(&context)
        .expect("register");
    node.store
        .handle()
        .put(
            &ContextMetaKey::new(context),
            &ContextMeta::new(ApplicationMeta::new(node.mine.app), [0; 32], vec![], None),
        )
        .expect("context meta");
    (sub, context)
}

#[actix::test]
async fn a_bound_key_cannot_move_its_group_under_another_application() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let (sub, _) = seed_subgroup(&node);

    let (status, body) = node
        .call(
            &token,
            Method::POST,
            &format!("/groups/{}/reparent", hex::encode(sub.to_bytes())),
            Some(json!({ "newParentId": hex::encode(node.theirs.namespace.to_bytes()) })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// A context binding reads up its group's chain but changes only its own group.
#[actix::test]
async fn a_context_bound_key_changes_only_its_own_group() {
    let node = Node::new().await;
    let (sub, context) = seed_subgroup(&node);
    let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
    permissions.push(format!("context[{context},identity]"));
    let token = node.client_token(permissions).await;
    let (ns, sub) = (
        hex::encode(node.mine.namespace.to_bytes()),
        hex::encode(sub.to_bytes()),
    );

    for (method, path) in [
        (Method::POST, format!("/groups/{ns}/members")),
        (Method::PUT, format!("/groups/{ns}/metadata")),
        (Method::DELETE, format!("/groups/{ns}")),
    ] {
        let (status, body) = node
            .call(&token, method.clone(), &path, Some(json!({})))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
    }
    let app = node.mine.app;
    for (path, body) in [
        ("/namespaces", json!({ "applicationId": app })),
        ("/groups", json!({ "applicationId": app })),
    ] {
        let (status, response) = node
            .call(&token, Method::POST, path, Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path} {body}: {response}");
    }
    for (method, path) in [
        (Method::GET, format!("/groups/{ns}/metadata")),
        (Method::PUT, format!("/groups/{sub}/metadata")),
    ] {
        let (status, body) = node
            .call(&token, method.clone(), &path, Some(json!({})))
            .await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
    }
}

#[actix::test]
async fn a_bound_key_is_refused_a_path_it_cannot_decode() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;

    let (status, body) = node.call(&token, Method::GET, "/contexts/%FF", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// A permissions header that names a binding never widens what an account session lists.
#[actix::test]
async fn an_account_session_behind_forward_auth_cannot_list_by_a_binding() {
    let node = Node::new().await;
    let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
    permissions.push(application_binding(&node.mine.app.to_string()));
    let request = Request::builder()
        .method(Method::GET)
        .uri("/admin-api/contexts")
        .header("X-Auth-Permissions", permissions.join(","))
        .header(
            "X-Auth-Account",
            calimero_account::AccountId::from([0x77; 32]).to_string(),
        );

    let (status, body) = Node::send(&node.proxied_with_identity, request, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(ids(&body, "contexts", "id").is_empty(), "{body}");
}

#[actix::test]
async fn a_context_bound_key_updates_its_own_contexts_application() {
    let node = Node::new().await;
    let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
    permissions.push("context:application:update".to_owned());
    permissions.push(format!("context[{},identity]", node.mine.context));
    let token = node.client_token(permissions).await;

    let (status, body) = node
        .call(
            &token,
            Method::POST,
            &format!("/contexts/{}/application", node.mine.context),
            Some(json!({
                "applicationId": node.mine.app,
                "executorPublicKey": calimero_primitives::identity::PublicKey::from([0x11; 32]),
            })),
        )
        .await;
    assert_ne!(status, StatusCode::FORBIDDEN, "{body}");
}
