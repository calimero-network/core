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
use wiremock::matchers::{body_json, header, method, path};
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
use calimero_server_primitives::admin::CreateGroupInvitationApiRequest;
use calimero_server_primitives::admin::CreateNamespaceApiRequest;
use calimero_server_primitives::admin::DeleteGroupApiRequest;
use calimero_server_primitives::admin::DeleteNamespaceApiRequest;
use calimero_server_primitives::admin::DetachContextFromGroupApiRequest;
use calimero_server_primitives::admin::GroupMemberApiInput;
use calimero_server_primitives::admin::JoinGroupApiRequest;
use calimero_server_primitives::admin::RegisterGroupSigningKeyApiRequest;
use calimero_server_primitives::admin::RemoveGroupMembersApiRequest;
use calimero_server_primitives::admin::ReparentGroupApiRequest;
use calimero_server_primitives::admin::ResyncContextApiRequest;
use calimero_server_primitives::admin::RetryGroupUpgradeApiRequest;
use calimero_server_primitives::admin::SetDefaultCapabilitiesApiRequest;
use calimero_server_primitives::admin::SetMemberCapabilitiesApiRequest;
use calimero_server_primitives::admin::SetSubgroupVisibilityApiRequest;
use calimero_server_primitives::admin::SyncGroupApiRequest;
use calimero_server_primitives::admin::UpdateContextApplicationRequest;
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
                "subgroupVisibility": "open",
                "metadata": { "name": null, "data": {}, "updatedAt": 0, "updatedBy": ZERO_BS58 },
                "groupStateHash": "0000000000000000000000000000000000000000000000000000000000000000"
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"members": []})))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.list_group_members(GID).await.unwrap();

    assert!(resp.members.is_empty());
}

#[tokio::test]
async fn add_group_members() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/groups/{GID}/members")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
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
async fn join_context() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/contexts/{CID}/join")))
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
    let resp = client.join_context(CID).await.unwrap();

    assert_eq!(resp.data.member_public_key, PublicKey::from([0u8; 32]));
}

// ---- Invitations & Joining ----

#[tokio::test]
async fn reparent_group() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/groups/{GID}/reparent")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"reparented": true})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .reparent_group(
            GID,
            ReparentGroupApiRequest {
                new_parent_id: "new-parent-id".to_owned(),
                requester: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn list_subgroups() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/admin-api/groups/{GID}/subgroups")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"subgroups": []})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.list_subgroups(GID).await.unwrap();
    assert!(resp.subgroups.is_empty());
}

// ---- Namespaces ----

#[tokio::test]
async fn list_namespaces() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/admin-api/namespaces"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.list_namespaces().await.unwrap();
    assert!(resp.data.is_empty());
}

#[tokio::test]
async fn create_namespace() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin-api/namespaces"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {"namespaceId": GID}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .create_namespace(CreateNamespaceApiRequest {
            application_id: ApplicationId::from([0u8; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            name: None,
            app_key: None,
        })
        .await
        .unwrap();
    assert_eq!(resp.data.namespace_id, GID);
}

#[tokio::test]
async fn get_namespace() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/admin-api/namespaces/{GID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "namespaceId": GID,
                "appKey": "testkey",
                "targetApplicationId": ZERO_BS58,
                "upgradePolicy": "Automatic",
                "createdAt": 0,
                "name": null,
                "memberCount": 0,
                "contextCount": 0,
                "subgroupCount": 0
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.get_namespace(GID).await.unwrap();
    assert_eq!(resp.namespace_id, GID);
}

#[tokio::test]
async fn delete_namespace() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!("/admin-api/namespaces/{GID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {"isDeleted": true}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .delete_namespace(GID, DeleteNamespaceApiRequest { requester: None })
        .await
        .unwrap();
    assert!(resp.data.is_deleted);
}

#[tokio::test]
async fn create_namespace_invitation() {
    let zeros: Vec<u8> = vec![0; 32];
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/namespaces/{GID}/invite")))
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
        .create_namespace_invitation(
            GID,
            CreateGroupInvitationApiRequest {
                requester: None,
                expiration_timestamp: None,
                recursive: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        resp["data"]["invitation"]["inviter_signature"],
        serde_json::Value::String("testsig".to_owned())
    );
}

#[tokio::test]
async fn join_namespace() {
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
        .and(path(format!("/admin-api/namespaces/{GID}/join")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {
                "groupId": GID,
                "memberIdentity": ZERO_BS58,
                "governanceOp": "deadbeef"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .join_namespace(
            GID,
            JoinGroupApiRequest {
                invitation,
                group_name: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(resp.data.group_id, GID);
}

#[tokio::test]
async fn list_namespace_groups() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/admin-api/namespaces/{GID}/groups")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.list_namespace_groups(GID).await.unwrap();
    assert!(resp.data.is_empty());
}

#[tokio::test]
async fn list_application_versions() {
    let server = MockServer::start().await;
    let app_id = ApplicationId::from([0u8; 32]);
    Mock::given(method("GET"))
        .and(path(format!("/admin-api/applications/{app_id}/versions")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"version": "2.0.0", "blobId": "blob-v2", "size": 2048, "package": "com.test.app"},
                {"version": "1.0.0", "blobId": "blob-v1", "size": 1024, "package": "com.test.app"}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.list_application_versions(&app_id).await.unwrap();
    assert_eq!(resp.data.len(), 2);
    assert_eq!(resp.data[0].version, "2.0.0");
    assert_eq!(resp.data[1].blob_id, "blob-v1");
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
                cascade: false,
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
async fn get_cascade_status() {
    let server = MockServer::start().await;
    // Non-empty entry exercises the nested CascadeStatusApiEntry +
    // GroupUpgradeStatusApiData deserialization (not just an empty array).
    Mock::given(method("GET"))
        .and(path(format!("/admin-api/groups/{GID}/cascade-status")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": [{
                "groupId": GID,
                "upgrade": {
                    "fromVersion": "1.0.0",
                    "toVersion": "2.0.0",
                    "initiatedAt": 100,
                    "initiatedBy": ZERO_BS58,
                    "status": "pending",
                    "total": 3,
                    "completed": 1,
                    "failed": 0
                },
                "cascadeHlc": "hlc-abc"
            }]})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.get_cascade_status(GID).await.unwrap();

    assert_eq!(resp.data.len(), 1);
    assert_eq!(resp.data[0].group_id, GID);
    assert_eq!(resp.data[0].upgrade.to_version, "2.0.0");
    assert_eq!(resp.data[0].cascade_hlc.as_deref(), Some("hlc-abc"));
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

#[tokio::test]
async fn get_migration_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/admin-api/groups/{GID}/migration-status")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "targetVersion": 2,
            "expectedMembers": 1,
            "rollup": {
                "migrated": 0,
                "inProgress": 0,
                "unknown": 0,
                "failed": 1,
                "total": 1,
                "allMigrated": false,
                "membersPendingSignature": 0
            },
            "members": [{
                "peer": ZERO_BS58,
                "report": {
                    "schemaVersion": 1,
                    "residueAuto": 0,
                    "residueIdentity": 2,
                    "syncedUpToHlc": 0,
                    "reportedAt": 100,
                    "authoredRemaining": 2,
                    "migrationFailed": "no_migration_path"
                },
                "state": "failed"
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.get_migration_status(GID).await.unwrap();

    assert_eq!(resp.target_version, 2);
    assert!(!resp.rollup.all_migrated);
    assert_eq!(resp.rollup.failed, 1);
    // Non-empty member array exercises the nested per-member DTO + report
    // deserialization (not just an empty rollup), including the stranded
    // `no_migration_path` failure reason.
    assert_eq!(resp.members.len(), 1);
    let report = resp.members[0]
        .report
        .as_ref()
        .expect("member report present");
    assert_eq!(report.authored_remaining, 2);
    assert_eq!(
        report.migration_failed.as_deref(),
        Some("no_migration_path")
    );
    assert_eq!(resp.members[0].state, "failed");
}

#[tokio::test]
async fn resync_context() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/contexts/{CID}/resync")))
        // Body assertion: the destructive `force` flag must actually be sent.
        .and(body_json(serde_json::json!({ "force": true })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "contextId": CID,
            "resyncStarted": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client
        .resync_context(CID, ResyncContextApiRequest { force: true })
        .await
        .unwrap();

    assert_eq!(resp.context_id, CID);
    assert!(resp.resync_started);
}

#[tokio::test]
async fn abort_migration() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/groups/{GID}/migration/abort")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "namespaceId": GID,
            "aborted": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let resp = client.abort_migration(GID).await.unwrap();

    assert_eq!(resp.namespace_id, GID);
    assert!(resp.aborted);
}

#[tokio::test]
async fn update_context_application() {
    let server = MockServer::start().await;
    // Format the same ContextId into both the mock path and the call, so the
    // match holds regardless of the id's Display encoding.
    let cid = ContextId::from([7u8; 32]);
    let app_id = ApplicationId::from([8u8; 32]);
    let executor = PublicKey::from([9u8; 32]);
    Mock::given(method("POST"))
        .and(path(format!("/admin-api/contexts/{cid}/application")))
        // `UpdateContextApplicationResponse.data` is the unit struct `Empty`,
        // which serializes as JSON `null`.
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": null })))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .update_context_application(&cid, UpdateContextApplicationRequest::new(app_id, executor))
        .await
        .unwrap();
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
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

// ---- Group Settings ----

#[tokio::test]
async fn set_default_capabilities() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/admin-api/groups/{GID}/settings/default-capabilities"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
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
async fn set_subgroup_visibility() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/admin-api/groups/{GID}/settings/subgroup-visibility"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    client
        .set_subgroup_visibility(
            GID,
            SetSubgroupVisibilityApiRequest {
                subgroup_visibility: "open".to_string(),
                requester: None,
            },
        )
        .await
        .unwrap();
}

// ---- Error handling ----

#[tokio::test]
async fn create_namespace_returns_err_on_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/admin-api/namespaces"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_json(serde_json::json!({"error": "internal server error"})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let result = client
        .create_namespace(CreateNamespaceApiRequest {
            application_id: ApplicationId::from([0u8; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            name: None,
            app_key: None,
        })
        .await;

    assert!(result.is_err());
}

// ---- Auth retry & query handling ----
//
// These exercise the connection's 401 handling with a real (in-memory) storage
// and authenticator, using `node_name = Some(..)` so the auth path runs.

use std::sync::{Arc, Mutex as StdMutex};

use wiremock::matchers::query_param;

use crate::connection::ConnectionInfo as Conn;

/// In-memory token storage seeded with an initial token.
#[derive(Clone)]
struct MemStorage {
    token: Arc<StdMutex<Option<JwtToken>>>,
}

#[async_trait]
impl ClientStorage for MemStorage {
    async fn load_tokens(&self, _: &str) -> Result<Option<JwtToken>> {
        Ok(self.token.lock().unwrap().clone())
    }

    async fn save_tokens(&self, _: &str, tokens: &JwtToken) -> Result<()> {
        *self.token.lock().unwrap() = Some(tokens.clone());
        Ok(())
    }
}

/// Authenticator that mints a fresh opaque token on demand. `authenticate` is
/// the fallback the connection uses when no refresh token is available.
#[derive(Clone)]
struct MemAuth {
    calls: Arc<StdMutex<u32>>,
}

#[async_trait]
impl ClientAuthenticator for MemAuth {
    async fn authenticate(&self, _: &Url) -> Result<JwtToken> {
        *self.calls.lock().unwrap() += 1;
        Ok(JwtToken::new("reauth-token".to_owned()))
    }

    async fn refresh_tokens(&self, _: &str) -> Result<JwtToken> {
        unimplemented!("not exercised")
    }

    async fn handle_auth_failure(&self, _: &Url) -> Result<JwtToken> {
        unimplemented!("not exercised")
    }

    async fn check_auth_required(&self, _: &Url) -> Result<bool> {
        Ok(true)
    }

    fn get_auth_method(&self) -> &'static str {
        "mem"
    }
}

fn make_auth_client(base_url: &Url) -> (Client<MemAuth, MemStorage>, Arc<StdMutex<u32>>) {
    // Seed an opaque (non-JWT, no refresh) token so a 401 forces the fallback
    // re-authentication path via `MemAuth::authenticate`.
    let storage = MemStorage {
        token: Arc::new(StdMutex::new(Some(JwtToken::new(
            "initial-token".to_owned(),
        )))),
    };
    let calls = Arc::new(StdMutex::new(0));
    let auth = MemAuth {
        calls: Arc::clone(&calls),
    };
    let conn = Conn::new(base_url.clone(), Some("node".to_owned()), auth, storage);
    (Client::new(conn).unwrap(), calls)
}

#[tokio::test]
async fn get_retries_on_401_and_reauthenticates() {
    let server = MockServer::start().await;

    // First GET → 401 (highest priority, only once), then 200.
    Mock::given(method("GET"))
        .and(path("/admin-api/contexts"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/admin-api/contexts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .with_priority(2)
        .expect(1)
        .mount(&server)
        .await;

    let (client, auth_calls) = make_auth_client(&Url::parse(&server.uri()).unwrap());
    let resp: serde_json::Value = client.connection().get("admin-api/contexts").await.unwrap();

    assert_eq!(resp["ok"], serde_json::Value::Bool(true));
    // Idempotent GET was retried after re-authenticating exactly once.
    assert_eq!(*auth_calls.lock().unwrap(), 1);
}

#[tokio::test]
async fn post_is_not_replayed_on_401() {
    let server = MockServer::start().await;

    // The POST endpoint always 401s. It must be hit exactly once — the client
    // must NOT replay a non-idempotent body after re-authenticating.
    Mock::given(method("POST"))
        .and(path("/admin-api/contexts"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;

    let (client, auth_calls) = make_auth_client(&Url::parse(&server.uri()).unwrap());
    let result: Result<serde_json::Value> = client
        .connection()
        .post("admin-api/contexts", serde_json::json!({"name": "x"}))
        .await;

    assert!(
        result.is_err(),
        "non-idempotent 401 should surface an error"
    );
    // The session was still re-authenticated so a manual retry would work.
    assert_eq!(*auth_calls.lock().unwrap(), 1);
}

#[tokio::test]
async fn request_forwards_query_string() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/admin-api/blobs"))
        .and(query_param("context_id", "ctx-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .expect(1)
        .mount(&server)
        .await;

    // node_name=None keeps auth out of the picture; this isolates the
    // path/query split in `request()`.
    let client = make_client(&Url::parse(&server.uri()).unwrap());
    let _resp: serde_json::Value = client
        .connection()
        .get("admin-api/blobs?context_id=ctx-123")
        .await
        .unwrap();
}

#[tokio::test]
async fn traversal_path_is_rejected_before_send() {
    // No server needed: the path guard rejects `..` before any request is built.
    let client = make_client(&Url::parse("https://unused.example/").unwrap());
    let result: Result<serde_json::Value> =
        client.connection().get("admin-api/groups/../evil").await;
    assert!(result.is_err());
}

// ---- Reverse-proxy base-path preservation ----
//
// When `api_url` carries a mount path (a node behind a reverse proxy), every
// endpoint the connection reaches — requests, token refresh, and the auth-mode
// probe — must stay under that base path rather than resetting to the host root.

#[tokio::test]
async fn detect_auth_mode_preserves_base_path() {
    let server = MockServer::start().await;
    // Only the base-path-prefixed probe is mocked → 200 = AuthMode::None. If the
    // base path were dropped the probe would hit `/admin-api/contexts`, which is
    // unmocked — wiremock answers unmatched routes with 404, and `detect_auth_mode`
    // maps a non-401 non-2xx status to AuthMode::Required. So asserting `None`
    // (not `Required`) proves the prefixed path was hit and returned 200.
    Mock::given(method("GET"))
        .and(path("/proxy/base/admin-api/contexts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .expect(1)
        .mount(&server)
        .await;

    let base = Url::parse(&format!("{}/proxy/base/", server.uri())).unwrap();
    let client = make_client(&base);
    let mode = client.connection().detect_auth_mode().await.unwrap();
    assert!(matches!(mode, crate::connection::AuthMode::None));
}

#[tokio::test]
async fn token_refresh_preserves_base_path() {
    let server = MockServer::start().await;

    // Protected endpoint under the base path: 401 once, then 200.
    Mock::given(method("GET"))
        .and(path("/proxy/base/admin-api/contexts"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/proxy/base/admin-api/contexts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .with_priority(2)
        .expect(1)
        .mount(&server)
        .await;

    // The refresh endpoint is mocked ONLY at the base-path-prefixed URL. If the
    // refresh used an absolute `/auth/refresh` it would miss this mock, the
    // refresh would fail, and the flow would fall back to interactive auth —
    // so `expect(1)` here (plus `authenticate` never being called) proves the
    // base path was preserved for the refresh POST.
    Mock::given(method("POST"))
        .and(path("/proxy/base/auth/refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": { "access_token": "new-access", "refresh_token": "new-refresh" }
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Seed a token that carries a refresh token so the 401 drives the refresh
    // path (not the interactive-auth fallback).
    let storage = MemStorage {
        token: Arc::new(StdMutex::new(Some(JwtToken::with_refresh(
            "initial-access".to_owned(),
            "refresh-tok".to_owned(),
        )))),
    };
    let auth_calls = Arc::new(StdMutex::new(0));
    let auth = MemAuth {
        calls: Arc::clone(&auth_calls),
    };
    let base = Url::parse(&format!("{}/proxy/base/", server.uri())).unwrap();
    let conn = Conn::new(base, Some("node".to_owned()), auth, storage);
    let client = Client::new(conn).unwrap();

    let resp: serde_json::Value = client.connection().get("admin-api/contexts").await.unwrap();

    assert_eq!(resp["ok"], serde_json::Value::Bool(true));
    // Refresh succeeded via the base-path URL, so interactive auth was not used.
    assert_eq!(*auth_calls.lock().unwrap(), 0);
}

// ---- Proactive-expiry refresh & single-flight ----
//
// End-to-end exercises of the auth state machine's expiry handling: a stored
// token whose JWT `exp` is in the past (or imminent) is refreshed *before* the
// request is sent, and concurrent expired requests collapse into a single
// `/auth/refresh`. These drive the real `ConnectionInfo` — `ensure_auth_header`
// → `refresh_or_reauth` (single-flight) → request — against a mock server.

use std::time::Duration;

use crate::test_support::jwt_with_exp;

/// Mount a `/auth/refresh` mock that returns `new_access` (+ a rotated refresh
/// token) and, separately, a protected GET that only matches when the request
/// carries `new_access` as its bearer. Together they prove the client refreshed
/// *before* issuing the GET: if it had sent the stale token, the GET would miss
/// the header-filtered mock and 404.
///
/// Both mocks assert their exact hit counts on server drop: `refresh_hits`
/// `/auth/refresh` POSTs and `get_hits` successful GETs carrying the fresh bearer.
/// `refresh_delay` holds the refresh response open, which (for the concurrent
/// test) keeps the single-flight lock held long enough that the other tasks are
/// guaranteed to queue behind it rather than each racing an instant refresh.
async fn mount_refresh_and_guarded_get(
    server: &MockServer,
    new_access: &str,
    refresh_hits: u64,
    get_hits: u64,
    refresh_delay: Duration,
) {
    Mock::given(method("POST"))
        .and(path("/auth/refresh"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(refresh_delay)
                .set_body_json(serde_json::json!({
                    "data": { "access_token": new_access, "refresh_token": "rotated-refresh" }
                })),
        )
        .expect(refresh_hits)
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/admin-api/contexts"))
        .and(header(
            "authorization",
            format!("Bearer {new_access}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .with_priority(1)
        .expect(get_hits)
        .mount(server)
        .await;

    // Any GET to the endpoint NOT carrying the fresh bearer means a stale token
    // was sent — exactly the regression these tests guard against. Match it with
    // a lower priority and `expect(0)` so it fails loudly with a descriptive body
    // instead of a bare unmatched-route 404.
    Mock::given(method("GET"))
        .and(path("/admin-api/contexts"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "stale bearer sent — proactive refresh / single-flight regressed"
        })))
        .with_priority(2)
        .expect(0)
        .mount(server)
        .await;
}

/// Build an auth-enabled client seeded with `token`, returning the `MemAuth`
/// call counter so a test can assert the interactive-auth fallback was **not**
/// used (i.e. the refresh path succeeded rather than falling back to
/// `authenticate`, which would otherwise mint a valid token and mask the bug).
fn auth_client_with_token(
    base_url: &Url,
    token: JwtToken,
) -> (Client<MemAuth, MemStorage>, Arc<StdMutex<u32>>) {
    let storage = MemStorage {
        token: Arc::new(StdMutex::new(Some(token))),
    };
    let calls = Arc::new(StdMutex::new(0));
    let auth = MemAuth {
        calls: Arc::clone(&calls),
    };
    let conn = Conn::new(base_url.clone(), Some("node".to_owned()), auth, storage);
    (Client::new(conn).unwrap(), calls)
}

#[tokio::test]
async fn expired_token_is_refreshed_before_request() {
    let now = chrono::Utc::now().timestamp();
    let expired = jwt_with_exp(now - 3600);
    let fresh = jwt_with_exp(now + 3600);

    let server = MockServer::start().await;
    // No 401 is mocked: a proactive refresh must happen *before* the GET, so the
    // only GET carries the fresh bearer. Exactly one `/auth/refresh`, one GET.
    mount_refresh_and_guarded_get(&server, &fresh, 1, 1, Duration::ZERO).await;

    let (client, auth_calls) = auth_client_with_token(
        &Url::parse(&server.uri()).unwrap(),
        JwtToken::with_refresh(expired, "refresh-tok".to_owned()),
    );

    let resp: serde_json::Value = client
        .connection()
        .get("admin-api/contexts")
        .await
        .unwrap_or_else(|e| panic!("stale bearer sent (proactive refresh regressed): {e}"));
    assert_eq!(resp["ok"], serde_json::Value::Bool(true));
    // Refresh path succeeded — no interactive-auth fallback.
    assert_eq!(*auth_calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn token_expiring_soon_is_refreshed_proactively() {
    let now = chrono::Utc::now().timestamp();
    // Not yet expired, but comfortably inside the proactive-refresh skew window.
    // Derived from the real constant so the test can't silently stop exercising
    // the `expires_soon` branch if the window is retuned.
    let expiring = jwt_with_exp(now + crate::connection::TOKEN_REFRESH_SKEW_SECS / 2);
    let fresh = jwt_with_exp(now + 3600);

    let server = MockServer::start().await;
    mount_refresh_and_guarded_get(&server, &fresh, 1, 1, Duration::ZERO).await;

    let (client, auth_calls) = auth_client_with_token(
        &Url::parse(&server.uri()).unwrap(),
        JwtToken::with_refresh(expiring, "refresh-tok".to_owned()),
    );

    let resp: serde_json::Value = client
        .connection()
        .get("admin-api/contexts")
        .await
        .unwrap_or_else(|e| panic!("stale bearer sent (proactive refresh regressed): {e}"));
    assert_eq!(resp["ok"], serde_json::Value::Bool(true));
    assert_eq!(*auth_calls.lock().unwrap(), 0);
}

// Pinned to the current-thread runtime so the cooperative-scheduling guarantee
// the body relies on is structural, not a default: on one thread the first task
// cannot complete its refresh without yielding at the network await (while
// holding `auth_lock`), which forces the other seven to block on the lock.
#[tokio::test(flavor = "current_thread")]
async fn concurrent_expired_requests_refresh_once() {
    let now = chrono::Utc::now().timestamp();
    let expired = jwt_with_exp(now - 3600);
    let fresh = jwt_with_exp(now + 3600);

    let server = MockServer::start().await;
    // Single-flight: 8 concurrent expired requests must produce exactly ONE
    // `/auth/refresh` (a rotating refresh token would be spent 8 times
    // otherwise) and 8 successful GETs carrying the fresh bearer.
    //
    // Contention is exercised, not assumed. On the pinned current-thread runtime
    // (see the attribute above): a `Barrier` releases all 8 tasks together, then the
    // first to run acquires `auth_lock` and parks on the refresh's network await
    // (held open 50 ms) while still holding the lock. Cooperative scheduling then
    // polls the other 7, which all block on `lock().await` before the refresh can
    // complete — so they hit the single-flight path rather than an already-done
    // early-exit. Structural backstops make a regression fail loudly regardless:
    // `expect(1)` on `/auth/refresh`, `expect(0)` on the stale-bearer GET.
    mount_refresh_and_guarded_get(&server, &fresh, 1, 8, Duration::from_millis(50)).await;

    let (client, auth_calls) = auth_client_with_token(
        &Url::parse(&server.uri()).unwrap(),
        JwtToken::with_refresh(expired, "refresh-tok".to_owned()),
    );

    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let mut set = tokio::task::JoinSet::new();
    for i in 0..8 {
        let client = client.clone();
        let barrier = Arc::clone(&barrier);
        set.spawn(async move {
            // Rendezvous so all 8 enter `ensure_auth_header` together.
            barrier.wait().await;
            let result = client
                .connection()
                .get::<serde_json::Value>("admin-api/contexts")
                .await;
            (i, result)
        });
    }

    let mut successes = 0;
    while let Some(joined) = set.join_next().await {
        let (i, result) = joined.expect("task should not panic");
        // A stale bearer (broken single-flight) hits the `expect(0)` catch-all →
        // 400 → Err; name the offending task in the failure.
        let body = result.unwrap_or_else(|e| {
            panic!("task {i} sent a stale bearer (single-flight regressed): {e}")
        });
        assert_eq!(body["ok"], serde_json::Value::Bool(true));
        successes += 1;
    }
    assert_eq!(successes, 8);
    assert_eq!(*auth_calls.lock().unwrap(), 0);
    // The refresh mock's `.expect(1)` and the fresh-bearer GET's `.expect(8)` are
    // verified on server drop, proving the eight concurrent 401-avoiding
    // refreshes collapsed into one while all eight requests succeeded.
}
