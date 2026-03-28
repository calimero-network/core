//! Unit tests for group API client methods.
//!
//! Each test verifies that the client fires the correct HTTP verb and URL
//! path, and that the response body is correctly deserialized.
//!
//! Auth is bypassed by setting `node_name: None` in `ConnectionInfo`, which
//! causes the auth path to be skipped entirely — `NoopAuth` and `NoopStorage`
//! are never actually called.

use async_trait::async_trait;
use eyre::Result;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::client::Client;
use crate::connection::ConnectionInfo;
use crate::storage::JwtToken;
use crate::traits::ClientAuthenticator;
use crate::traits::ClientStorage;

use calimero_context_config::types::SignedGroupOpenInvitation;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::context::UpgradePolicy;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::AddGroupMembersApiRequest;
use calimero_server_primitives::admin::CreateGroupApiRequest;
use calimero_server_primitives::admin::CreateGroupInvitationApiRequest;
use calimero_server_primitives::admin::DeleteGroupApiRequest;
use calimero_server_primitives::admin::DetachContextFromGroupApiRequest;
use calimero_server_primitives::admin::GroupMemberApiInput;
use calimero_server_primitives::admin::JoinGroupApiRequest;
use calimero_server_primitives::admin::JoinGroupContextApiRequest;
use calimero_server_primitives::admin::ManageContextAllowlistApiRequest;
use calimero_server_primitives::admin::RegisterGroupSigningKeyApiRequest;
use calimero_server_primitives::admin::RemoveGroupMembersApiRequest;
use calimero_server_primitives::admin::RetryGroupUpgradeApiRequest;
use calimero_server_primitives::admin::SetContextVisibilityApiRequest;
use calimero_server_primitives::admin::SetDefaultCapabilitiesApiRequest;
use calimero_server_primitives::admin::SetDefaultVisibilityApiRequest;
use calimero_server_primitives::admin::SetMemberCapabilitiesApiRequest;
use calimero_server_primitives::admin::SyncGroupApiRequest;
use calimero_server_primitives::admin::UpdateGroupSettingsApiRequest;
use calimero_server_primitives::admin::UpdateMemberRoleApiRequest;
use calimero_server_primitives::admin::UpgradeGroupApiRequest;

/// Fixed test group ID used across tests.
const GID: &str = "test-group-id";

/// Fixed test context ID used in multi-segment path tests.
const CID: &str = "test-ctx-id";

/// Fixed test identity hex used in member path segments.
const IDENT: &str = "test-ident";

/// Base58 encoding of `[0u8; 32]` — used wherever a Hash-backed type is
/// required in JSON response bodies.
const ZERO_BS58: &str = "11111111111111111111111111111111";

// ---- Stub impls (node_name=None means these are never called) ----

#[derive(Clone)]
struct NoopAuth;

#[derive(Clone)]
struct NoopStorage;

#[async_trait]
impl ClientAuthenticator for NoopAuth {
    async fn authenticate(&self, _: &Url) -> Result<JwtToken> {
        unimplemented!("NoopAuth is never called when node_name=None")
    }

    async fn refresh_tokens(&self, _: &str) -> Result<JwtToken> {
        unimplemented!("NoopAuth is never called when node_name=None")
    }

    async fn handle_auth_failure(&self, _: &Url) -> Result<JwtToken> {
        unimplemented!("NoopAuth is never called when node_name=None")
    }

    async fn check_auth_required(&self, _: &Url) -> Result<bool> {
        unimplemented!("NoopAuth is never called when node_name=None")
    }

    fn get_auth_method(&self) -> &'static str {
        "noop"
    }
}

#[async_trait]
impl ClientStorage for NoopStorage {
    async fn load_tokens(&self, _: &str) -> Result<Option<JwtToken>> {
        unimplemented!("NoopStorage is never called when node_name=None")
    }

    async fn save_tokens(&self, _: &str, _: &JwtToken) -> Result<()> {
        unimplemented!("NoopStorage is never called when node_name=None")
    }
}

fn make_client(base_url: &Url) -> Client<NoopAuth, NoopStorage> {
    let conn = ConnectionInfo::new(base_url.clone(), None, NoopAuth, NoopStorage);
    Client::new(conn).unwrap()
}

// ---- Group CRUD ----

#[tokio::test]
async fn list_groups() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/admin-api/groups"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.list_groups().await.unwrap();

    assert!(resp.data.is_empty());
}

#[tokio::test]
async fn create_group() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin-api/groups"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {"groupId": GID}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .create_group(CreateGroupApiRequest {
            group_id: None,
            app_key: None,
            application_id: ApplicationId::from([0u8; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            alias: None,
            parent_group_id: None,
        })
        .await
        .unwrap();

    assert_eq!(resp.data.group_id, GID);
}

#[tokio::test]
async fn get_group_info() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/admin-api/groups/{GID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "groupId": GID,
                "appKey": "testkey",
                "targetApplicationId": ZERO_BS58,
                "upgradePolicy": "Automatic",
                "memberCount": 0,
                "contextCount": 0,
                "activeUpgrade": null,
                "defaultCapabilities": 0,
                "defaultVisibility": "open"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.get_group_info(GID).await.unwrap();

    assert_eq!(resp.data.group_id, GID);
}

#[tokio::test]
async fn delete_group() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!("/admin-api/groups/{GID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {"isDeleted": true}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .delete_group(GID, DeleteGroupApiRequest { requester: None })
        .await
        .unwrap();

    assert!(resp.data.is_deleted);
}

#[tokio::test]
async fn update_group_settings() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(format!("/admin-api/groups/{GID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::Value::Null))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .update_group_settings(
            GID,
            UpdateGroupSettingsApiRequest {
                requester: None,
                upgrade_policy: UpgradePolicy::Automatic,
            },
        )
        .await
        .unwrap();
}

// ---- Members ----

#[tokio::test]
async fn list_group_members() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/admin-api/groups/{GID}/members")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.list_group_members(GID).await.unwrap();

    assert!(resp.data.is_empty());
}

#[tokio::test]
async fn add_group_members() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/groups/{GID}/members")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::Value::Null))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .add_group_members(
            GID,
            AddGroupMembersApiRequest {
                members: vec![GroupMemberApiInput {
                    identity: PublicKey::from([0u8; 32]),
                    role: GroupMemberRole::Member,
                }],
                requester: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn remove_group_members() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/groups/{GID}/members/remove")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::Value::Null))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .remove_group_members(
            GID,
            RemoveGroupMembersApiRequest {
                members: vec![PublicKey::from([0u8; 32])],
                requester: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn update_member_role() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/admin-api/groups/{GID}/members/{IDENT}/role"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::Value::Null))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .update_member_role(
            GID,
            IDENT,
            UpdateMemberRoleApiRequest {
                role: GroupMemberRole::Admin,
                requester: None,
            },
        )
        .await
        .unwrap();
}

// ---- Contexts ----

#[tokio::test]
async fn list_group_contexts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/admin-api/groups/{GID}/contexts")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.list_group_contexts(GID).await.unwrap();

    assert!(resp.data.is_empty());
}

#[tokio::test]
async fn detach_context_from_group() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/admin-api/groups/{GID}/contexts/{CID}/remove"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::Value::Null))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .detach_context_from_group(
            GID,
            CID,
            DetachContextFromGroupApiRequest { requester: None },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn join_group_context() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/groups/{GID}/join-context")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "contextId": ZERO_BS58,
                "memberPublicKey": ZERO_BS58
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .join_group_context(
            GID,
            JoinGroupContextApiRequest {
                context_id: ContextId::from([0u8; 32]),
            },
        )
        .await
        .unwrap();

    assert_eq!(resp.data.member_public_key, PublicKey::from([0u8; 32]));
}

// ---- Invitations & Joining ----

#[tokio::test]
async fn create_group_invitation() {
    // `SignedGroupOpenInvitation` fields are snake_case (no rename_all on that
    // struct), so the mock body uses the raw field names from the types crate.
    let zeros: Vec<u8> = vec![0; 32];
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/groups/{GID}/invite")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "invitation": {
                    "invitation": {
                        "inviter_identity": zeros,
                        "group_id": zeros,
                        "expiration_timestamp": 0u64,
                        "secret_salt": zeros
                    },
                    "inviter_signature": "testsig"
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .create_group_invitation(
            GID,
            CreateGroupInvitationApiRequest {
                requester: None,
                expiration_timestamp: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(resp.data.invitation.inviter_signature, "testsig");
}

#[tokio::test]
async fn join_group() {
    // Build the invitation by deserializing from JSON so we avoid private-field
    // construction of the inner Identity/SignerId newtypes.
    let zeros: Vec<u8> = vec![0; 32];
    let invitation: SignedGroupOpenInvitation = serde_json::from_value(serde_json::json!({
        "invitation": {
            "inviter_identity": zeros,
            "group_id": zeros,
            "expiration_timestamp": 0u64,
            "secret_salt": zeros
        },
        "inviter_signature": "testsig"
    }))
    .unwrap();

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin-api/groups/join"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "groupId": GID,
                "memberIdentity": ZERO_BS58
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .join_group(JoinGroupApiRequest {
            invitation,
            group_alias: None,
        })
        .await
        .unwrap();

    assert_eq!(resp.data.group_id, GID);
}

// ---- Upgrade ----

#[tokio::test]
async fn upgrade_group() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/groups/{GID}/upgrade")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "groupId": GID,
                "status": "pending",
                "total": null,
                "completed": null,
                "failed": null
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .upgrade_group(
            GID,
            UpgradeGroupApiRequest {
                target_application_id: ApplicationId::from([0u8; 32]),
                requester: None,
                migrate_method: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(resp.data.group_id, GID);
}

#[tokio::test]
async fn get_group_upgrade_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/admin-api/groups/{GID}/upgrade/status")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": null})))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.get_group_upgrade_status(GID).await.unwrap();

    assert!(resp.data.is_none());
}

#[tokio::test]
async fn retry_group_upgrade() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/groups/{GID}/upgrade/retry")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "groupId": GID,
                "status": "pending",
                "total": null,
                "completed": null,
                "failed": null
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .retry_group_upgrade(GID, RetryGroupUpgradeApiRequest { requester: None })
        .await
        .unwrap();

    assert_eq!(resp.data.group_id, GID);
}

// ---- Sync & Signing Key ----

#[tokio::test]
async fn sync_group() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/groups/{GID}/sync")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "groupId": GID,
                "appKey": "testkey",
                "targetApplicationId": ZERO_BS58,
                "memberCount": 0,
                "contextCount": 0
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .sync_group(GID, SyncGroupApiRequest { requester: None })
        .await
        .unwrap();

    assert_eq!(resp.data.group_id, GID);
}

#[tokio::test]
async fn register_group_signing_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/groups/{GID}/signing-key")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {"publicKey": ZERO_BS58}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .register_group_signing_key(
            GID,
            RegisterGroupSigningKeyApiRequest {
                signing_key: "testkey".to_string(),
            },
        )
        .await
        .unwrap();

    assert_eq!(resp.data.public_key, PublicKey::from([0u8; 32]));
}

// ---- Member Capabilities & Visibility ----

#[tokio::test]
async fn set_member_capabilities() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/admin-api/groups/{GID}/members/{IDENT}/capabilities"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::Value::Null))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .set_member_capabilities(
            GID,
            IDENT,
            SetMemberCapabilitiesApiRequest {
                capabilities: 0,
                requester: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn get_member_capabilities() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/admin-api/groups/{GID}/members/{IDENT}/capabilities"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {"capabilities": 42}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.get_member_capabilities(GID, IDENT).await.unwrap();

    assert_eq!(resp.data.capabilities, 42);
}

#[tokio::test]
async fn set_context_visibility() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/admin-api/groups/{GID}/contexts/{CID}/visibility"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::Value::Null))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .set_context_visibility(
            GID,
            CID,
            SetContextVisibilityApiRequest {
                mode: "open".to_string(),
                requester: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn get_context_visibility() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/admin-api/groups/{GID}/contexts/{CID}/visibility"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "mode": "open",
                "creator": ZERO_BS58
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.get_context_visibility(GID, CID).await.unwrap();

    assert_eq!(resp.data.mode, "open");
}

// ---- Allowlist ----

#[tokio::test]
async fn manage_context_allowlist() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/admin-api/groups/{GID}/contexts/{CID}/allowlist"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::Value::Null))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .manage_context_allowlist(
            GID,
            CID,
            ManageContextAllowlistApiRequest {
                add: vec![],
                remove: vec![],
                requester: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn get_context_allowlist() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/admin-api/groups/{GID}/contexts/{CID}/allowlist"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.get_context_allowlist(GID, CID).await.unwrap();

    assert!(resp.data.is_empty());
}

// ---- Group Settings ----

#[tokio::test]
async fn set_default_capabilities() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/admin-api/groups/{GID}/settings/default-capabilities"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::Value::Null))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .set_default_capabilities(
            GID,
            SetDefaultCapabilitiesApiRequest {
                default_capabilities: 0,
                requester: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn set_default_visibility() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/admin-api/groups/{GID}/settings/default-visibility"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::Value::Null))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .set_default_visibility(
            GID,
            SetDefaultVisibilityApiRequest {
                default_visibility: "open".to_string(),
                requester: None,
            },
        )
        .await
        .unwrap();
}

// ---- Error handling ----

#[tokio::test]
async fn create_group_returns_err_on_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin-api/groups"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_json(serde_json::json!({"error": "internal server error"})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let result = client
        .create_group(CreateGroupApiRequest {
            group_id: None,
            app_key: None,
            application_id: ApplicationId::from([0u8; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            alias: None,
            parent_group_id: None,
        })
        .await;

    assert!(result.is_err());
}
