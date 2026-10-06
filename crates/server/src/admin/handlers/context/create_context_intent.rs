//! `GET`/`POST /admin-api/groups/:group_id/context-intents` — create a context a
//! member authorized, and tell a keyholder what it needs before authorizing one.
//!
//! # Why this exists
//!
//! `POST .../contexts/:id/intents` runs a member's write in a context that
//! already exists. An account with no node of its own also has to be able to
//! make that context in the first place — a chat channel, a DM, a document — and
//! the relay that carries its writes cannot do it as itself: a relay is not a
//! context creator, and a TEE relay's own writes are read-only by its role. So
//! the member signs a `ContextCreationWarrant` pinning the group, the seed, the
//! application and the `init` arguments, and this node carries it out. Every
//! peer checks the member's `CAN_CREATE_CONTEXT`, never this node's.
//!
//! # What is checked here and nowhere else
//!
//! **`not_after`**, for the reason `perform_intent` gives: this is the only
//! place one clock decides, before anything has converged.
//!
//! **That the warrant covers these `init` arguments.** The warrant commits to
//! them as a hash, and only the party holding the plaintext can compare.
//!
//! Everything else — the author's authority, this node's standing, what the
//! registration may name — is the governance gate's, which runs once here
//! before `init` and again on every peer when the registration applies.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_server_primitives::admin::{
    CreateContextIntentApiRequest, CreateContextIntentApiResponse,
    CreateContextIntentApiResponseData, CreateContextIntentRelayApiResponse,
    CreateContextIntentRelayApiResponseData,
};
use eyre::WrapErr as _;
use tracing::{debug, error, warn};

use crate::admin::handlers::context::perform_intent::{
    decode_author_proof, now_secs, IntentRefusal,
};
use crate::admin::handlers::identity::get_node_identity::node_identity;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// Parse a hex group id from the path, as a 400 on failure.
pub(crate) fn parse_group_id(raw: &str) -> Result<ContextGroupId, ApiError> {
    let bytes = hex::decode(raw).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("group id '{raw}' is not hex"),
    })?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("group id '{raw}' is not 32 bytes"),
    })?;
    Ok(ContextGroupId::from(bytes))
}

/// Decode the hex-borsh creation warrant a client sent, as a 400 on failure.
pub(crate) fn decode_creation_warrant(
    hex_warrant: &str,
) -> eyre::Result<calimero_account::ContextCreationWarrant> {
    let bytes = hex::decode(hex_warrant.trim()).map_err(|err| {
        eyre::eyre!(IntentRefusal::Malformed(format!(
            "warrant is not hex: {err}"
        )))
    })?;
    borsh::from_slice(&bytes).map_err(|err| {
        eyre::eyre!(IntentRefusal::Malformed(format!(
            "warrant is not a valid context creation warrant ({} bytes): {err}",
            bytes.len()
        )))
    })
}

/// The checks only this node can make: the warrant is for this group, covers
/// these `init` arguments, and has not expired.
///
/// Order matters for the same reason it does on `/intents`: a warrant for
/// another group is refused as such, before the clock is consulted, so a client
/// is not told to re-mint something it should not have sent here at all.
pub(crate) fn creation_warrant_authorises(
    warrant: &calimero_account::ContextCreationWarrant,
    group_id: &ContextGroupId,
    init_args: &[u8],
    now: u64,
) -> eyre::Result<()> {
    if warrant.group != group_id.to_bytes() {
        eyre::bail!(IntentRefusal::NotAuthorized(
            "this creation warrant authorises a different group than the one it was \
             presented in"
                .to_owned()
        ));
    }
    if !warrant.covers_init(init_args) {
        eyre::bail!(IntentRefusal::NotAuthorized(
            "this creation warrant does not cover the init arguments presented with it".to_owned()
        ));
    }
    if warrant.not_after < now {
        eyre::bail!(IntentRefusal::NotAuthorized(format!(
            "this creation warrant expired at {} and it is now {now}; mint a fresh one",
            warrant.not_after
        )));
    }
    Ok(())
}

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<CreateContextIntentApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    match perform(&state, group_id, req).await {
        Ok(data) => ApiResponse {
            payload: CreateContextIntentApiResponse { data },
        }
        .into_response(),
        Err(err) => {
            warn!(group_id = %group_id_str, %err, "refusing context creation intent");
            parse_api_error(err).into_response()
        }
    }
}

async fn perform(
    state: &AdminState,
    group_id: ContextGroupId,
    req: CreateContextIntentApiRequest,
) -> eyre::Result<CreateContextIntentApiResponseData> {
    let warrant = decode_creation_warrant(&req.warrant)?;

    let author_proof = decode_author_proof(&req.author_proof)?;

    let init_args = serde_json::to_vec(&req.init_args)
        .map_err(|err| eyre::eyre!("init arguments could not be encoded: {err}"))?;
    creation_warrant_authorises(&warrant, &group_id, &init_args, now_secs())?;

    let store = state.ctx_client.datastore();
    // This node signs the registration as its identity in the namespace, so
    // that is the key the bundle has to certify.
    let (signer, _secret) = calimero_governance_store::NamespaceRepository::new(store)
        .resolve_identity(&group_id)?
        .ok_or_else(|| calimero_context::error::ContextError::NotAGroupMember {
            group_id: group_id.to_string(),
        })?;
    let executor_proof = calimero_context::join_credential::build(store, &group_id, &signer)
        .wrap_err("this node could not present its own credential")?;

    let delegation = calimero_account::ContextCreationDelegation {
        warrant: Box::new(warrant),
        author_proof: Box::new(author_proof),
        executor_proof,
        executor_key: signer,
    };
    // Authenticity before anything else, so a forged bundle is a 400 about the
    // bundle and never a 403 about a capability it could not have claimed.
    let verified = delegation.verify().map_err(|err| {
        eyre::eyre!(IntentRefusal::Malformed(format!(
            "creation delegation does not verify: {err}"
        )))
    })?;

    debug!(
        %group_id,
        author = %verified.author_account,
        nonce = verified.nonce,
        "creating a context on a member's behalf"
    );

    let created = state
        .ctx_client
        .create_context_on_behalf(delegation, init_args)
        .await?;

    Ok(CreateContextIntentApiResponseData {
        context_id: created.context_id,
        group_id: hex::encode(group_id.to_bytes()),
        member_public_key: created.identity,
    })
}

/// `GET`: the executor account and key to name, and whether this node may carry a
/// member's creation here; with `?author=`, whether that account may create.
pub async fn describe_handler(
    Path(group_id_str): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let author = match query
        .get("author")
        .map(|raw| parse_account(raw))
        .transpose()
    {
        Ok(author) => author,
        Err(err) => return err.into_response(),
    };

    let store = state.ctx_client.datastore();

    match calimero_governance_store::MetaRepository::new(store).load(&group_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "this node knows no such group".to_owned(),
            }
            .into_response()
        }
        Err(err) => {
            error!(error = ?err, %group_id, "Failed to read the group");
            return internal("Failed to read the group");
        }
    }

    let executor_account = match node_identity(store) {
        Ok(Some((account, ..))) => account,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "this node holds neither a usable device nor an account root yet, \
                          so it can be named as no warrant's executor"
                    .to_owned(),
            }
            .into_response()
        }
        Err(err) => {
            error!(error = ?err, "Failed to read this node's identity");
            return internal("Failed to read this node's identity");
        }
    };

    let executor_key = match node_signing_key(store) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    let can_create_on_behalf =
        match calimero_governance_store::warrant_gate::executor_refusal_for_group(
            store,
            &group_id,
            executor_account,
        ) {
            Ok(refusal) => refusal.is_none(),
            Err(err) => {
                error!(error = ?err, %group_id, "Failed to read this node's standing");
                return internal("Failed to read this node's standing");
            }
        };

    let author_may_create = match author {
        None => None,
        Some(author) => match author_may_create(store, &group_id, &author) {
            Ok(may) => Some(may),
            Err(err) => {
                error!(error = ?err, %group_id, "Failed to read the author's standing");
                return internal("Failed to read the author's standing");
            }
        },
    };

    ApiResponse {
        payload: CreateContextIntentRelayApiResponse {
            data: CreateContextIntentRelayApiResponseData {
                executor_account: hex::encode(executor_account.as_bytes()),
                executor_key,
                group_id: hex::encode(group_id.to_bytes()),
                can_create_on_behalf,
                author_may_create,
            },
        },
    }
    .into_response()
}

/// Whether `author` may create here as this node currently sees the group: a
/// member that is not read-only, holding admin or `CAN_CREATE_CONTEXT`. The
/// live view, so a hint rather than the decision — the registration gate decides
/// at the op's cut on every peer.
fn author_may_create(
    store: &calimero_store::Store,
    group_id: &ContextGroupId,
    author: &calimero_account::AccountId,
) -> eyre::Result<bool> {
    let permissions = calimero_governance_store::PermissionChecker::new(store, *group_id);
    let role = calimero_governance_store::MembershipRepository::new(store)
        .effective_role(group_id, author)?;
    let member = match role {
        Some((role, _)) => !role.is_read_only(),
        None => permissions.is_admin_account(author)?,
    };
    Ok(member
        && permissions.is_account_authorized_with_capability(
            author,
            MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
        )?)
}

fn parse_account(raw: &str) -> Result<calimero_account::AccountId, ApiError> {
    let bytes = hex::decode(raw).map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("author '{raw}' is not hex"),
    })?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        message: format!("author '{raw}' is not 32 bytes"),
    })?;
    Ok(calimero_account::AccountId::from(bytes))
}

/// The key this node signs with, the one a warrant for it must name as its
/// `executor_key`, or the response saying why it cannot be read.
pub(crate) fn node_signing_key(
    store: &calimero_store::Store,
) -> Result<calimero_primitives::identity::PublicKey, Box<axum::response::Response>> {
    match calimero_governance_store::NamespaceRepository::new(store).node_identity() {
        Ok(Some(identity)) => Ok(identity.public_key),
        Ok(None) => Err(Box::new(
            ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "this node holds no signing key yet".to_owned(),
            }
            .into_response(),
        )),
        Err(err) => {
            error!(error = ?err, "Failed to read this node's signing key");
            Err(Box::new(internal("Failed to read this node's signing key")))
        }
    }
}

pub(crate) fn internal(message: &str) -> axum::response::Response {
    ApiError {
        status_code: StatusCode::INTERNAL_SERVER_ERROR,
        message: message.to_owned(),
    }
    .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use axum::{Extension, Router};
    use calimero_account::{ContextCreationTerms, ContextCreationWarrant};
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::NamespaceRepository;
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::identity::{AccountId, PrivateKey, PublicKey};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use tower::ServiceExt as _;

    use super::{creation_warrant_authorises, decode_creation_warrant, parse_group_id};
    use crate::admin::handlers::context::perform_intent::IntentRefusal;

    const GROUP: [u8; 32] = [0x11; 32];
    const ARGS: &[u8] = br#"{"name":"general"}"#;
    const NOW: u64 = 1_700_000_000;

    fn warrant(not_after: u64) -> ContextCreationWarrant {
        ContextCreationWarrant::sign(
            &PrivateKey::from([7u8; 32]),
            ContextCreationTerms {
                group: GROUP,
                seed: [0x12; 32],
                author_account: AccountId::from([0x22; 32]),
                executor: AccountId::from([0x33; 32]),
                executor_key: PrivateKey::from([0x34; 32]).public_key(),
                application_id: ApplicationId::from([0x44; 32]),
                service_name: None,
                name: Some("general".to_owned()),
                init_hash: ContextCreationWarrant::init_hash(ARGS),
                account_heads: vec![],
                governance_floor: vec![],
                nonce: 1,
                not_after,
            },
        )
        .expect("sign")
    }

    /// A creation warrant that never expires, naming `executor_key` as the
    /// device that may carry it out.
    fn warrant_naming(_executor_key: PublicKey) -> ContextCreationWarrant {
        warrant(u64::MAX)
    }

    /// A creation warrant naming another executor device is refused as not
    /// this node's to carry out, before it presents a credential or runs `init`.
    #[actix::test]
    async fn a_creation_warrant_naming_another_executor_key_is_a_403() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (state, _blobs) = crate::test_support::admin_state(&store).await;
        let namespaces = NamespaceRepository::new(&store);
        let _signer = namespaces
            .provision_node_identity()
            .expect("provision the signing key");
        let _participation = namespaces
            .participate_in(&ContextGroupId::from(GROUP))
            .expect("take part in the group");
        let app = Router::new()
            .route("/groups/{group_id}/context-intents", post(super::handler))
            .layer(Extension(state));
        let warrant = warrant_naming(PrivateKey::from([0x3D; 32]).public_key());
        let body = serde_json::json!({
            "warrant": hex::encode(borsh::to_vec(&warrant).expect("borsh")),
            "authorProof": crate::test_support::author_proof_hex(),
            "initArgs": serde_json::from_slice::<serde_json::Value>(ARGS).expect("json args"),
        });
        let request = Request::post(format!("/groups/{}/context-intents", hex::encode(GROUP)))
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("a request");

        let response =
            tokio::time::timeout(std::time::Duration::from_secs(2), app.oneshot(request))
                .await
                .expect("refused before anything waits on the context actor")
                .expect("the route answers");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read the response");
        let body = String::from_utf8_lossy(&body);
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(body.contains("executor key"), "{body}");
    }

    /// A store holding `GROUP`, and the public router over it.
    async fn discovery(provision_key: bool) -> (Store, Router, tempfile::TempDir) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        calimero_governance_store::MetaRepository::new(&store)
            .save(
                &ContextGroupId::from(GROUP),
                &calimero_governance_store::test_fixtures::test_meta(),
            )
            .expect("save the group");
        if provision_key {
            let _key = NamespaceRepository::new(&store)
                .provision_node_identity()
                .expect("provision the signing key");
        }
        let (router, blobs) = crate::test_support::public_router(&store).await;
        (store, router, blobs)
    }

    /// Discovery names the key a creation warrant for this node must carry.
    #[actix::test]
    async fn discovery_names_this_nodes_signing_key() {
        let (store, router, _blobs) = discovery(true).await;
        let key = NamespaceRepository::new(&store)
            .node_identity()
            .expect("read the key")
            .expect("provisioned")
            .public_key;

        let (status, body) = crate::test_support::get(
            router,
            &format!("/groups/{}/context-intents", hex::encode(GROUP)),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(
            body.contains(&format!("\"executorKey\":\"{key}\"")),
            "{body}"
        );
    }

    /// A node with no signing key can be named in no warrant, and says so.
    #[actix::test]
    async fn discovery_without_a_signing_key_is_a_404() {
        let (_store, router, _blobs) = discovery(false).await;

        let (status, body) = crate::test_support::get(
            router,
            &format!("/groups/{}/context-intents", hex::encode(GROUP)),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }

    fn refusal(err: &eyre::Report) -> String {
        err.downcast_ref::<IntentRefusal>().map_or_else(
            || format!("not an IntentRefusal: {err}"),
            ToString::to_string,
        )
    }

    #[test]
    fn a_live_warrant_for_its_own_group_and_args_is_authorised() {
        creation_warrant_authorises(&warrant(NOW + 60), &ContextGroupId::from(GROUP), ARGS, NOW)
            .expect("must authorise");
    }

    #[test]
    fn expiring_exactly_now_still_authorises() {
        creation_warrant_authorises(&warrant(NOW), &ContextGroupId::from(GROUP), ARGS, NOW)
            .expect("not_after is the last live second");
    }

    #[test]
    fn an_expired_warrant_is_refused() {
        let err =
            creation_warrant_authorises(&warrant(NOW - 1), &ContextGroupId::from(GROUP), ARGS, NOW)
                .expect_err("expired");
        assert!(refusal(&err).contains("expired"), "{}", refusal(&err));
        assert!(matches!(
            err.downcast_ref::<IntentRefusal>(),
            Some(IntentRefusal::NotAuthorized(_))
        ));
    }

    #[test]
    fn a_warrant_for_another_group_is_refused_before_the_clock_matters() {
        let err = creation_warrant_authorises(
            &warrant(NOW - 1),
            &ContextGroupId::from([0x99; 32]),
            ARGS,
            NOW,
        )
        .expect_err("another group");
        let msg = refusal(&err);
        assert!(msg.contains("different group"), "{msg}");
        assert!(!msg.contains("expired"), "expiry must not shadow it: {msg}");
    }

    #[test]
    fn other_init_arguments_are_refused() {
        let err = creation_warrant_authorises(
            &warrant(NOW + 60),
            &ContextGroupId::from(GROUP),
            br#"{"name":"random"}"#,
            NOW,
        )
        .expect_err("other args");
        assert!(
            refusal(&err).contains("does not cover"),
            "{}",
            refusal(&err)
        );
    }

    /// The server hashes `serde_json::to_vec` of the JSON it received, which is
    /// what a JS client hashes with `JSON.stringify` for a single-key object.
    #[test]
    fn init_args_are_hashed_as_the_server_re_encodes_them() {
        let value: serde_json::Value = serde_json::from_slice(ARGS).expect("json");
        let bytes = serde_json::to_vec(&value).expect("encode");
        assert_eq!(bytes, ARGS);
        creation_warrant_authorises(
            &warrant(NOW + 60),
            &ContextGroupId::from(GROUP),
            &bytes,
            NOW,
        )
        .expect("re-encoded args must still be covered");
    }

    #[test]
    fn a_warrant_that_is_not_hex_or_not_borsh_is_malformed() {
        for bad in ["zz", "00", ""] {
            let err = decode_creation_warrant(bad).expect_err("malformed");
            assert!(
                matches!(
                    err.downcast_ref::<IntentRefusal>(),
                    Some(IntentRefusal::Malformed(_))
                ),
                "{bad:?}: {err}"
            );
        }
    }

    /// A METHOD warrant is not a creation warrant: its bytes must not decode as
    /// one, or a relay could present consent to a write as consent to create.
    #[test]
    fn a_method_warrant_does_not_decode_as_a_creation_warrant() {
        let method = calimero_account::Warrant::sign(
            &PrivateKey::from([7u8; 32]),
            calimero_account::WarrantTerms {
                context: calimero_primitives::context::ContextId::from([0x12; 32]),
                author_account: AccountId::from([0x22; 32]),
                executor: AccountId::from([0x33; 32]),
                executor_key: PrivateKey::from([0x34; 32]).public_key(),
                app_version: ApplicationId::from([0x44; 32]),
                method: "init".to_owned(),
                intent_hash: [0u8; 32],
                account_heads: vec![],
                governance_floor: vec![],
                nonce: 1,
                not_after: NOW,
            },
        )
        .expect("sign");
        let hex_method = hex::encode(borsh::to_vec(&method).expect("encode"));
        match decode_creation_warrant(&hex_method) {
            Err(_) => {}
            Ok(decoded) => {
                let _refused = decoded
                    .verify_signature()
                    .expect_err("a method warrant's signature must not verify as a creation");
            }
        }
    }

    #[test]
    fn a_round_tripped_warrant_decodes_to_itself() {
        let w = warrant(NOW);
        let hex_w = hex::encode(borsh::to_vec(&w).expect("encode"));
        assert_eq!(decode_creation_warrant(&hex_w).expect("decode"), w);
    }

    #[test]
    fn group_ids_must_be_32_bytes_of_hex() {
        assert!(parse_group_id(&"11".repeat(32)).is_ok());
        assert!(parse_group_id(&"11".repeat(31)).is_err());
        assert!(parse_group_id("not-hex").is_err());
    }
}
