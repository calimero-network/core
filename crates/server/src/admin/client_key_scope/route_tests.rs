//! A bound client key against the real admin router and auth guard, on a node
//! holding two applications' namespaces, contexts and blobs.

use std::sync::Arc;

use actix::{Actor, Context as ActixContext, Handler};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use calimero_context_client::client::ContextClient;
use calimero_context_client::group::{GroupContextEntry, NamespaceSummary};
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_client::messages::ContextMessage;
use calimero_context_config::types::BytecodeId;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::test_fixtures::enrol_member;
use calimero_governance_store::{
    apply_local_signed_group_op, ContextTreeService, MembershipRepository, MetaRepository,
    NamespaceRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::db::Column;
use calimero_store::db::InMemoryDB;
use calimero_store::key::{
    ApplicationMeta, AsKeyParts as _, ContextMeta as ContextMetaKey, GroupMeta, GroupMetaValue,
};
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

use super::GROUP_REFUSAL;
use crate::admin::service::{setup, AdminConfig};
use crate::app_host::{encode_label, serve_app_hosts, AppHosts, AppOrigins};
use crate::config::{AppsConfig, ServerConfig};
use crate::service_mounts::with_optional_auth;
use crate::{AdminState, NodeReadiness};

const NAMESPACE: [u8; 32] = [0xc1; 32]; // the namespace the join tests target

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

/// A join body whose unsigned invitation for `group` hints at `app`.
fn invitation_hinting(group: &ContextGroupId, app: &ApplicationId) -> Value {
    let mut invitation = calimero_context::test_support::unverified_invitation(group);
    invitation.application_id = Some(*app.as_ref());
    json!({ "invitation": invitation })
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
            target_release: None,
            created_at: meta.created_at,
            name: None,
            member_count: 0,
            context_count: 0,
            subgroup_count: 0,
            forgotten: false,
        })
        .collect()
}

struct Node {
    router: Router,
    app_hosts: Router,
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
        let hosts = AppHosts {
            node_client: node_client.clone(),
            origins: AppOrigins::new(&AppsConfig::default()),
            app_api: true,
        };
        let app_hosts = router
            .clone()
            .layer(axum::middleware::from_fn_with_state(hosts, serve_app_hosts));

        Self {
            router,
            app_hosts,
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

    /// A request to `app`'s own origin, which the app host answers before the router.
    async fn call_on_origin_of(
        &self,
        app: &ApplicationId,
        token: Option<&str>,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("/admin-api{path}"))
            .header("Host", format!("{}.localhost:2428", encode_label(app)));
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        Self::send(&self.app_hosts, request, body).await
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
/// against: the invitation's application is an unsigned hint, taken when it names the bound one.
#[actix::test]
async fn a_bound_key_joins_a_group_this_node_does_not_hold_only_on_a_hint_of_its_application() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let unknown = ContextGroupId::from(NAMESPACE);
    let unknown_hex = hex::encode(unknown.to_bytes());

    for path in [
        format!("/namespaces/{unknown_hex}/join"),
        "/groups/join".to_owned(),
    ] {
        let (status, body) = node
            .call(
                &token,
                Method::POST,
                &path,
                Some(invitation_hinting(&unknown, &node.theirs.app)),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{path} hinting theirs: {body}"
        );
        let (status, body) = node
            .call(
                &token,
                Method::POST,
                &path,
                Some(invitation_hinting(&unknown, &node.mine.app)),
            )
            .await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{path} hinting mine: {body}");
    }
    // A rejoin names no invitation; it is held to the same rule by the path alone.
    for namespace in [unknown, node.theirs.namespace] {
        let path = format!("/namespaces/{}/join", hex::encode(namespace.to_bytes()));
        let (status, body) = node
            .call(&token, Method::POST, &path, Some(json!({})))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
    }
    let (status, body) = node
        .call(
            &token,
            Method::POST,
            "/groups/join",
            Some(invitation_hinting(&node.mine.namespace, &node.mine.app)),
        )
        .await;
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "a held group of mine: {body}"
    );
}

#[actix::test]
async fn a_context_bound_key_cannot_join_a_group_this_node_does_not_hold() {
    let node = Node::new().await;
    let token = node
        .client_token(vec![
            "namespace".to_owned(),
            format!("context[{},identity]", node.mine.context),
            application_binding(&node.mine.app.to_string()),
        ])
        .await;
    let unknown = ContextGroupId::from(NAMESPACE);

    let (status, body) = node
        .call(
            &token,
            Method::POST,
            &format!("/namespaces/{}/join", hex::encode(unknown.to_bytes())),
            Some(invitation_hinting(&unknown, &node.mine.app)),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// The namespace as the join's first phase leaves it: the invitation's hint as
/// target, marked unverified, and the placeholder as admin.
fn seed_unverified(node: &Node, hint: ApplicationId) -> ContextGroupId {
    let namespace = ContextGroupId::from(NAMESPACE);
    let placeholder = calimero_governance_store::placeholder_admin_identity();
    MetaRepository::new(&node.store)
        .save(
            &namespace,
            &GroupMetaValue {
                target: calimero_store::key::GroupTarget {
                    application_id: hint,
                    ..Default::default()
                },
                admin_identity: placeholder,
                owner_identity: placeholder,
                migration: None,
                created_at: 0,
                auto_join: true,
            },
        )
        .expect("the join's first phase");
    MetaRepository::new(&node.store)
        .mark_target_unverified(&namespace)
        .expect("mark the hint");
    namespace
}

/// Genesis names the namespace's admin and applies no target op.
fn genesis(node: &Node, namespace: &ContextGroupId) -> PrivateKey {
    let admin_sk = PrivateKey::from([0x77; 32]);
    let admin = enrol_member(&node.store, namespace, &admin_sk.public_key());
    let mut meta = MetaRepository::new(&node.store)
        .load(namespace)
        .expect("read the meta")
        .expect("the meta");
    meta.admin_identity = admin;
    meta.owner_identity = admin;
    MetaRepository::new(&node.store)
        .save(namespace, &meta)
        .expect("genesis");
    MembershipRepository::new(&node.store)
        .add_member(namespace, &admin, GroupMemberRole::Admin)
        .expect("the admin");
    admin_sk
}

/// Genesis names the namespace's admin, and the admin's target op applies.
fn found_with_target(node: &Node, namespace: &ContextGroupId, target: ApplicationId) {
    let admin_sk = genesis(node, namespace);
    let op = SignedGroupOp::sign(
        &admin_sk,
        namespace.to_bytes().into(),
        vec![],
        1,
        GroupOp::TargetApplicationSet {
            bytecode_id: BytecodeId::from([0xaa; 32]),
            target_application_id: target,
            package: "com.example.app".to_owned(),
            version: "1.0.0".to_owned(),
        },
    )
    .expect("sign the target op");
    apply_local_signed_group_op(&node.store, &op).expect("the target op applies");
}

/// Every group-level route a key reaches a namespace by.
fn namespace_routes(
    namespace: &ContextGroupId,
    app: &ApplicationId,
) -> Vec<(Method, String, Value)> {
    let ns = hex::encode(namespace.to_bytes());
    vec![
        (Method::GET, format!("/groups/{ns}/members"), json!({})),
        (Method::POST, format!("/namespaces/{ns}/invite"), json!({})),
        (Method::POST, format!("/namespaces/{ns}/groups"), json!({})),
        (
            Method::POST,
            "/contexts".to_owned(),
            json!({ "applicationId": app, "groupId": ns, "initializationParams": [] }),
        ),
    ]
}

async fn listed_namespaces(node: &Node, token: &str, namespace: &ContextGroupId) -> Vec<bool> {
    let mut seen = Vec::new();
    for path in [
        "/namespaces".to_owned(),
        format!("/namespaces/for-application/{}", node.mine.app),
    ] {
        let (status, body) = node.call(token, Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        seen.push(
            ids(&body, "namespaces", "namespaceId").contains(&hex::encode(namespace.to_bytes())),
        );
    }
    seen
}

async fn assert_routes(
    node: &Node,
    token: &str,
    namespace: &ContextGroupId,
    refused: bool,
    when: &str,
) {
    for (method, path, body) in namespace_routes(namespace, &node.mine.app) {
        let (status, response) = node.call(token, method.clone(), &path, Some(body)).await;
        if refused {
            assert_eq!(status, StatusCode::FORBIDDEN, "{when}: {method} {path}");
            assert_eq!(response["error"], GROUP_REFUSAL, "{when}: {method} {path}");
        } else {
            assert_ne!(
                status,
                StatusCode::FORBIDDEN,
                "{when}: {method} {path}: {response}"
            );
        }
    }
    let listed = listed_namespaces(node, token, namespace).await;
    assert_eq!(listed, vec![!refused; 2], "{when}: listings");
}

/// The invitation's application is an unsigned hint, so a namespace still
/// seeded from it gives an application's key no scope but the join.
#[actix::test]
async fn an_app_bound_key_cannot_act_on_a_namespace_whose_signed_admin_has_not_arrived() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let namespace = seed_unverified(&node, node.mine.app);

    assert_routes(&node, &token, &namespace, true, "before genesis").await;
    let (status, body) = node
        .call(
            &token,
            Method::POST,
            &format!("/namespaces/{}/join", hex::encode(namespace.to_bytes())),
            Some(invitation_hinting(&namespace, &node.mine.app)),
        )
        .await;
    assert_ne!(status, StatusCode::FORBIDDEN, "the join: {body}");
}

/// Genesis alone does not confirm a hinted application: only a signed op that
/// names it does.
#[actix::test]
async fn an_app_bound_key_cannot_act_on_a_namespace_whose_hinted_application_no_signed_op_confirmed(
) {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let namespace = seed_unverified(&node, node.mine.app);

    genesis(&node, &namespace);
    assert_routes(&node, &token, &namespace, true, "after genesis").await;
}

/// A subgroup created under a marked namespace copies its hinted target and has
/// a real admin, but carries no marker of its own.
#[actix::test]
async fn an_app_bound_key_cannot_act_on_a_subgroup_of_a_namespace_with_a_hinted_application() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let namespace = seed_unverified(&node, node.mine.app);
    genesis(&node, &namespace);

    let subgroup = ContextGroupId::from([0xc2; 32]);
    let meta = MetaRepository::new(&node.store)
        .load(&namespace)
        .expect("read")
        .expect("the namespace meta");
    MetaRepository::new(&node.store)
        .save(&subgroup, &meta)
        .expect("the subgroup meta");
    NamespaceRepository::new(&node.store)
        .nest(&namespace, &subgroup)
        .expect("nest");
    let path = format!("/groups/{}/members", hex::encode(subgroup.to_bytes()));

    let (status, body) = node.call(&token, Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "before the target: {body}");

    found_with_target(&node, &namespace, node.mine.app);
    let (status, body) = node.call(&token, Method::GET, &path, None).await;
    assert_ne!(status, StatusCode::FORBIDDEN, "after the target: {body}");
}

#[actix::test]
async fn a_hinted_join_is_refused_when_the_group_meta_cannot_be_read() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let namespace = seed_unverified(&node, node.mine.app);
    let key = GroupMeta::new(namespace.to_bytes());
    let raw: &[u8; 33] = key.as_key().as_ref();
    node.store
        .raw_put(Column::Group, raw, b"not a meta")
        .expect("corrupt the row");

    let (status, body) = node
        .call(
            &token,
            Method::POST,
            "/groups/join",
            Some(invitation_hinting(&namespace, &node.mine.app)),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[actix::test]
async fn an_app_bound_key_acts_on_a_namespace_once_the_real_target_is_its_application() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let namespace = seed_unverified(&node, node.mine.app);

    found_with_target(&node, &namespace, node.mine.app);
    assert_routes(&node, &token, &namespace, false, "after the target is mine").await;
}

#[actix::test]
async fn a_key_bound_to_a_hinted_application_loses_the_namespace_when_the_real_target_lands() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let namespace = seed_unverified(&node, node.mine.app);

    found_with_target(&node, &namespace, node.theirs.app);
    assert_routes(
        &node,
        &token,
        &namespace,
        true,
        "after the target is theirs",
    )
    .await;
}

#[actix::test]
async fn an_unbound_key_acts_on_a_namespace_whose_admin_has_not_arrived() {
    let node = Node::new().await;
    let token = node.unbound().await;
    let namespace = seed_unverified(&node, node.theirs.app);

    for (method, path, body) in namespace_routes(&namespace, &node.mine.app) {
        let (status, response) = node.call(&token, method.clone(), &path, Some(body)).await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{method} {path}: {response}");
    }
}

#[actix::test]
async fn an_app_origin_joins_a_namespace_this_node_does_not_hold_only_on_a_hint_of_its_app() {
    let node = Node::new().await;
    let token = node.unbound().await;
    let unknown = ContextGroupId::from(NAMESPACE);
    let path = format!("/namespaces/{}/join", hex::encode(unknown.to_bytes()));

    for (hint, joins) in [(node.mine.app, true), (node.theirs.app, false)] {
        let (status, body) = node
            .call_on_origin_of(
                &node.mine.app,
                Some(&token),
                Method::POST,
                &path,
                Some(invitation_hinting(&unknown, &hint)),
            )
            .await;
        assert_eq!(status != StatusCode::FORBIDDEN, joins, "{hint}: {body}");
    }
}

#[actix::test]
async fn a_bound_key_restores_only_its_own_applications_removed_namespace() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;
    let (mine, theirs) = (
        ContextGroupId::from([0xc2; 32]),
        ContextGroupId::from([0xc3; 32]),
    );
    let removals = calimero_governance_store::ForgottenNamespaces::new(&node.store);
    removals
        .mark(&mine, Some(node.mine.app), 1, None)
        .expect("this application's removal");
    removals
        .mark(&theirs, Some(node.theirs.app), 1, None)
        .expect("another application's removal");

    for (namespace, permitted) in [(mine, true), (theirs, false)] {
        let path = format!("/namespaces/{}/join", hex::encode(namespace.to_bytes()));
        let (status, body) = node
            .call(&token, Method::POST, &path, Some(json!({})))
            .await;
        assert_eq!(status != StatusCode::FORBIDDEN, permitted, "{path}: {body}");
    }
}

#[actix::test]
async fn a_bound_key_rejoins_a_held_namespace_only_of_its_own_application() {
    let node = Node::new().await;
    let token = node.bound_to_mine().await;

    for (namespace, permitted) in [(node.mine.namespace, true), (node.theirs.namespace, false)] {
        let path = format!("/namespaces/{}/join", hex::encode(namespace.to_bytes()));
        let (status, body) = node
            .call(&token, Method::POST, &path, Some(json!({})))
            .await;
        assert_eq!(status != StatusCode::FORBIDDEN, permitted, "{path}: {body}");
    }
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

    // Their alias answers as an unset one does, and survives.
    let (status, _) = node
        .call(&token, Method::POST, "/alias/delete/context/theirs", None)
        .await;
    assert_eq!(status, StatusCode::OK, "deleting their alias");
    let (_, body) = node
        .call(&owner, Method::POST, "/alias/lookup/context/theirs", None)
        .await;
    assert_eq!(
        body["data"]["value"],
        json!(node.theirs.context.to_string()),
        "{body}"
    );
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
    let unknown = ContextGroupId::from(NAMESPACE);
    let unknown_hex = hex::encode(unknown.to_bytes());

    let (status, body) = node
        .call(
            &token,
            Method::POST,
            "/groups/join",
            Some(invitation_hinting(&node.theirs.namespace, &node.theirs.app)),
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
            Some(invitation_hinting(&node.theirs.namespace, &node.mine.app)),
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
            Some(invitation_hinting(&unknown, &node.theirs.app)),
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

/// An account session whose permissions carry a binding lists the intersection of both.
#[actix::test]
async fn an_account_session_with_a_binding_lists_only_its_bound_groups() {
    let node = Node::new().await;
    let account = calimero_account::AccountId::from([0x77; 32]);
    for namespace in [node.mine.namespace, node.theirs.namespace] {
        calimero_governance_store::MembershipRepository::new(&node.store)
            .add_member(
                &namespace,
                &account,
                calimero_primitives::context::GroupMemberRole::Member,
            )
            .expect("member");
    }
    let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
    permissions.push(application_binding(&node.mine.app.to_string()));
    let request = Request::builder()
        .method(Method::GET)
        .uri("/admin-api/contexts")
        .header("X-Auth-Permissions", permissions.join(","))
        .header("X-Auth-Account", account.to_string());

    let (status, body) = Node::send(&node.proxied_with_identity, request, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids(&body, "contexts", "id"),
        vec![node.mine.context.to_string()]
    );
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

/// A context binding on an account session lists only that context, in every listing.
#[actix::test]
async fn an_account_session_with_a_context_binding_lists_only_its_context() {
    let node = Node::new().await;
    let sibling = ContextId::from([0xa0; 32]);
    ContextTreeService::new(&node.store, node.mine.namespace)
        .register_context(&sibling)
        .expect("register");
    let account = calimero_account::AccountId::from([0x77; 32]);
    calimero_governance_store::MembershipRepository::new(&node.store)
        .add_member(
            &node.mine.namespace,
            &account,
            calimero_primitives::context::GroupMemberRole::Member,
        )
        .expect("member");
    let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
    permissions.push(format!("context[{},identity]", node.mine.context));
    let ns = hex::encode(node.mine.namespace.to_bytes());

    for (path, field) in [
        ("/admin-api/contexts".to_owned(), "id"),
        (format!("/admin-api/groups/{ns}/contexts"), "contextId"),
    ] {
        let request = Request::builder()
            .method(Method::GET)
            .uri(&path)
            .header("X-Auth-Permissions", permissions.join(","))
            .header("X-Auth-Account", account.to_string());
        let (status, body) = Node::send(&node.proxied_with_identity, request, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        assert_eq!(
            ids(&body, "contexts", field),
            vec![node.mine.context.to_string()],
            "{path}"
        );
    }
}

#[actix::test]
async fn an_app_origin_lists_only_its_own_applications_contexts_and_namespaces() {
    let node = Node::new().await;
    let token = node.unbound().await;

    for tenant in [&node.mine, &node.theirs] {
        let (status, body) = node
            .call_on_origin_of(&tenant.app, Some(&token), Method::GET, "/contexts", None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            ids(&body, "contexts", "id"),
            vec![tenant.context.to_string()]
        );

        let (status, body) = node
            .call_on_origin_of(&tenant.app, Some(&token), Method::GET, "/namespaces", None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            ids(&body, "namespaces", "namespaceId"),
            vec![hex::encode(tenant.namespace.to_bytes())]
        );
    }
}

#[actix::test]
async fn an_app_origin_cannot_reach_another_applications_namespace_or_context() {
    let node = Node::new().await;
    let token = node.unbound().await;
    let ns = hex::encode(node.theirs.namespace.to_bytes());
    let theirs = node.theirs.app;
    let join = invitation_hinting(&node.theirs.namespace, &node.mine.app);
    let (status, body) = node
        .call_on_origin_of(
            &node.mine.app,
            Some(&token),
            Method::POST,
            &format!("/namespaces/{ns}/join"),
            Some(join),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a held namespace relabelled: {body}"
    );

    for (method, path) in [
        (Method::POST, format!("/namespaces/{ns}/invite")),
        (Method::POST, format!("/namespaces/{ns}/groups")),
        (Method::GET, format!("/groups/{ns}/members")),
        (Method::GET, format!("/contexts/for-application/{theirs}")),
        (Method::GET, format!("/namespaces/for-application/{theirs}")),
        (Method::GET, format!("/applications/{theirs}")),
        (Method::GET, format!("/contexts/{}", node.theirs.context)),
        (
            Method::GET,
            format!("/contexts/{}/group", node.theirs.context),
        ),
    ] {
        let (status, body) = node
            .call_on_origin_of(
                &node.mine.app,
                Some(&token),
                method.clone(),
                &path,
                Some(json!({})),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
    }
}

#[actix::test]
async fn an_app_origin_reaches_its_own_applications_routes() {
    let node = Node::new().await;
    seed_application(&node, node.mine.app, node.mine.blob);
    let token = node.unbound().await;
    let own = node.mine.app;

    for path in [
        format!("/contexts/{}", node.mine.context),
        format!("/contexts/for-application/{own}"),
        format!("/namespaces/for-application/{own}"),
        format!("/applications/{own}"),
        format!("/contexts/{}/group", node.mine.context),
    ] {
        let (status, body) = node
            .call_on_origin_of(&own, Some(&token), Method::GET, &path, None)
            .await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }
}

#[actix::test]
async fn an_app_origin_refuses_an_admin_token_and_a_token_bound_to_another_app() {
    let node = Node::new().await;
    let admin = node
        .tokens
        .generate_token_pair("root".to_owned(), vec!["admin".to_owned()], None, None)
        .await
        .unwrap()
        .0;
    let theirs = node
        .client_token({
            let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
            permissions.push(application_binding(&node.theirs.app.to_string()));
            permissions
        })
        .await;

    for token in [&admin, &theirs] {
        let (status, body) = node
            .call_on_origin_of(&node.mine.app, Some(token), Method::GET, "/contexts", None)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
    let (status, _) = node.call(&admin, Method::GET, "/contexts", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the admin token still works on the node's own origin"
    );
}

#[actix::test]
async fn an_app_origin_needs_a_token_and_refuses_the_rest_of_the_admin_api() {
    let node = Node::new().await;
    let token = node.unbound().await;
    let blob = node.mine.blob;
    let context = node.mine.context;

    let (status, _) = node
        .call_on_origin_of(&node.mine.app, None, Method::GET, "/contexts", None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for (method, path) in [
        (Method::DELETE, format!("/blobs/{blob}")),
        (Method::PUT, format!("/contexts/{context}")),
        (Method::GET, "/blobs".to_owned()),
        (Method::POST, "/install-application".to_owned()),
        (Method::GET, format!("/applications/{}/abi", node.mine.app)),
    ] {
        let (status, _) = node
            .call_on_origin_of(&node.mine.app, Some(&token), method.clone(), &path, None)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
    }
}

#[actix::test]
async fn an_app_origin_lists_only_its_own_application() {
    let node = Node::new().await;
    seed_application(&node, node.mine.app, node.mine.blob);
    seed_application(&node, node.theirs.app, node.theirs.blob);
    let token = node.unbound().await;

    for tenant in [&node.mine, &node.theirs] {
        let (status, body) = node
            .call_on_origin_of(
                &tenant.app,
                Some(&token),
                Method::GET,
                "/applications",
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(ids(&body, "apps", "id"), vec![tenant.app.to_string()]);
    }
}

#[actix::test]
async fn an_unbound_key_on_the_nodes_own_host_lists_every_application() {
    let node = Node::new().await;
    seed_application(&node, node.mine.app, node.mine.blob);
    seed_application(&node, node.theirs.app, node.theirs.blob);
    let token = node.unbound().await;

    let (status, body) = node.call(&token, Method::GET, "/applications", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body, "apps", "id").len(), 2, "{body}");
}

#[actix::test]
async fn an_app_origin_cannot_create_in_another_application() {
    let node = Node::new().await;
    let token = node.unbound().await;
    let theirs = node.theirs.app;
    let (their_ns, my_ns) = (
        hex::encode(node.theirs.namespace.to_bytes()),
        hex::encode(node.mine.namespace.to_bytes()),
    );

    for (path, body) in [
        (
            "/contexts",
            json!({ "applicationId": theirs, "groupId": their_ns, "initializationParams": [] }),
        ),
        (
            "/contexts",
            json!({ "applicationId": theirs, "groupId": my_ns, "initializationParams": [] }),
        ),
        ("/namespaces", json!({ "applicationId": theirs })),
    ] {
        let (status, response) = node
            .call_on_origin_of(
                &node.mine.app,
                Some(&token),
                Method::POST,
                path,
                Some(body.clone()),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path} {body}: {response}");
    }
}

/// Each route an app host passes on, as (method, path) with `{g}` the group, `{c}` the context and `{a}` an account.
const WIDENED_ROUTES: &[(Method, &str)] = &[
    (Method::GET, "/groups/{g}"),
    (Method::GET, "/groups/{g}/metadata"),
    (Method::PUT, "/groups/{g}/metadata"),
    (Method::PUT, "/groups/{g}/contexts/{c}/metadata"),
    (Method::POST, "/groups/{g}/members"),
    (Method::POST, "/groups/{g}/members/remove"),
    (Method::PUT, "/groups/{g}/members/{a}/role"),
    (Method::PUT, "/groups/{g}/members/{a}/capabilities"),
    (Method::PUT, "/groups/{g}/settings/default-capabilities"),
    (Method::PUT, "/groups/{g}/settings/subgroup-visibility"),
    (Method::GET, "/groups/{g}/subgroups"),
    (Method::GET, "/groups/{g}/contexts"),
    (Method::GET, "/groups/{g}/members/{a}/metadata"),
    (Method::PUT, "/groups/{g}/members/{a}/metadata"),
    (Method::GET, "/namespaces/{g}"),
    (Method::GET, "/namespaces/{g}/groups"),
    (Method::POST, "/groups/{g}/reparent"),
    (Method::POST, "/groups/{g}/sync"),
    (Method::POST, "/groups/{g}/leave"),
    (Method::DELETE, "/groups/{g}"),
    (Method::DELETE, "/namespaces/{g}"),
    (Method::DELETE, "/contexts/{c}"),
    (Method::POST, "/contexts/{c}/leave"),
    (Method::GET, "/contexts/{c}/identities"),
];

#[actix::test]
async fn an_app_origin_reaches_the_widened_routes_for_its_own_resources_only() {
    let node = Node::new().await;
    let mut permissions: Vec<String> = APP_TOKEN.iter().map(|p| (*p).to_owned()).collect();
    permissions.extend(["context:delete".to_owned(), "context:leave".to_owned()]);
    let token = node.client_token(permissions).await;
    let account = hex::encode([0x33; 32]);
    let fill = |template: &str, tenant: &Tenant| {
        template
            .replace("{g}", &hex::encode(tenant.namespace.to_bytes()))
            .replace("{c}", &tenant.context.to_string())
            .replace("{a}", &account)
    };

    for (method, template) in WIDENED_ROUTES {
        let own = fill(template, &node.mine);
        let (status, body) = node
            .call_on_origin_of(
                &node.mine.app,
                Some(&token),
                method.clone(),
                &own,
                Some(json!({})),
            )
            .await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{method} {own}: {body}");

        let theirs = fill(template, &node.theirs);
        let (status, body) = node
            .call_on_origin_of(
                &node.mine.app,
                Some(&token),
                method.clone(),
                &theirs,
                Some(json!({})),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {theirs}: {body}");
        assert!(
            body.to_string().contains("not permitted"),
            "{method} {theirs} is refused by the binding, not the host: {body}"
        );
    }
}

#[actix::test]
async fn an_app_origin_reads_the_nodes_own_identity() {
    let node = Node::new().await;
    let token = node.unbound().await;
    let (status, body) = node
        .call_on_origin_of(&node.mine.app, Some(&token), Method::GET, "/identity", None)
        .await;
    assert_ne!(status, StatusCode::FORBIDDEN, "{body}");
}

#[actix::test]
async fn an_app_origin_joins_groups_aliases_contexts_and_blobs_for_its_own_app_only() {
    let node = Node::new().await;
    let token = node.unbound().await;
    let (mine, theirs) = (&node.mine, &node.theirs);
    let call = async |method: Method, path: String, body: Option<Value>| {
        node.call_on_origin_of(&mine.app, Some(&token), method, &path, body)
            .await
    };

    let (status, body) = call(
        Method::POST,
        "/groups/join".to_owned(),
        Some(invitation_hinting(&theirs.namespace, &mine.app)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "joining their group: {body}");

    let alias = |context: &Tenant| json!({ "alias": "x", "contextId": context.context });
    let (status, body) = call(
        Method::POST,
        "/alias/create/context".to_owned(),
        Some(alias(theirs)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "aliasing theirs: {body}");
    let (status, body) = call(
        Method::POST,
        "/alias/create/context".to_owned(),
        Some(alias(mine)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "aliasing mine: {body}");
    let (status, body) = call(Method::POST, "/alias/lookup/context/x".to_owned(), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["value"], json!(mine.context.to_string()));

    let reparent = json!({ "newParentId": hex::encode(theirs.namespace.to_bytes()) });
    let (status, body) = call(
        Method::POST,
        format!(
            "/groups/{}/reparent",
            hex::encode(mine.namespace.to_bytes())
        ),
        Some(reparent),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "reparenting under theirs: {body}"
    );

    for (tenant, expected) in [(mine, StatusCode::OK), (theirs, StatusCode::FORBIDDEN)] {
        let uri = format!("/blobs/{}?context_id={}", mine.blob, tenant.context);
        let (status, body) = call(Method::GET, uri, None).await;
        assert_eq!(
            status, expected,
            "reading a blob through {}: {body}",
            tenant.context
        );
    }
    let (status, body) = call(Method::GET, format!("/blobs/{}", theirs.blob), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no context named: {body}");
    let (status, body) = call(
        Method::PUT,
        format!("/blobs?context_id={}", theirs.context),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "uploading into theirs: {body}"
    );
}

/// A subgroup of `mine`'s namespace targeting `target`, holding a context that runs `context_app`.
fn seed_mixed_subgroup(node: &Node, target: ApplicationId, context_app: Option<ApplicationId>) {
    let sub = ContextGroupId::from([0xa4; 32]);
    let mut meta = MetaRepository::new(&node.store)
        .load(&node.mine.namespace)
        .expect("load")
        .expect("namespace meta");
    meta.target.application_id = target;
    MetaRepository::new(&node.store)
        .save(&sub, &meta)
        .expect("meta");
    calimero_governance_store::NamespaceRepository::new(&node.store)
        .nest(&node.mine.namespace, &sub)
        .expect("nest");
    if let Some(app) = context_app {
        let context = ContextId::from([0xa5; 32]);
        ContextTreeService::new(&node.store, sub)
            .register_context(&context)
            .expect("register");
        node.store
            .handle()
            .put(
                &ContextMetaKey::new(context),
                &ContextMeta::new(ApplicationMeta::new(app), [0; 32], vec![], None),
            )
            .expect("context meta");
    }
}

#[actix::test]
async fn an_app_origin_cannot_cascade_over_a_subtree_holding_another_applications_groups_or_contexts(
) {
    let account = hex::encode([0x33; 32]);
    for (foreign_group, foreign_context) in [(true, false), (false, true)] {
        let node = Node::new().await;
        let (mine, theirs) = (node.mine.app, node.theirs.app);
        if foreign_group {
            seed_mixed_subgroup(&node, theirs, None);
        } else {
            seed_mixed_subgroup(&node, mine, Some(theirs));
        }
        let token = node.unbound().await;
        let ns = hex::encode(node.mine.namespace.to_bytes());

        for (method, path) in [
            (Method::DELETE, format!("/groups/{ns}")),
            (Method::DELETE, format!("/namespaces/{ns}")),
            (Method::POST, format!("/groups/{ns}/members/remove")),
            (Method::PUT, format!("/groups/{ns}/members/{account}/role")),
            (
                Method::PUT,
                format!("/groups/{ns}/members/{account}/capabilities"),
            ),
        ] {
            let (status, body) = node
                .call_on_origin_of(&mine, Some(&token), method.clone(), &path, Some(json!({})))
                .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {path} {foreign_group} {foreign_context}: {body}"
            );
        }
    }

    let node = Node::new().await;
    seed_mixed_subgroup(&node, node.mine.app, Some(node.mine.app));
    let token = node.unbound().await;
    let ns = hex::encode(node.mine.namespace.to_bytes());
    let (status, body) = node
        .call_on_origin_of(
            &node.mine.app,
            Some(&token),
            Method::DELETE,
            &format!("/groups/{ns}"),
            Some(json!({})),
        )
        .await;
    assert_ne!(status, StatusCode::FORBIDDEN, "a tree of one app: {body}");
}
