//! `GET`/`POST /admin-api/groups/:group_id/governance-intents` — publish one
//! governance op a member authorized, and tell a keyholder what it needs first.
//!
//! The governance counterpart of `/contexts/:id/intents` and
//! `/groups/:id/context-intents`: an account with no node signs a
//! `GovernanceWarrant` over the exact op — adding the other person to a DM,
//! creating a channel's subgroup, renaming it — and this node publishes it. Every
//! peer applies the op as the member, so the member's own authority decides.
//!
//! Checked here and nowhere else: `not_after`, and that the op sent is the one
//! the warrant commits to and is one a relay may carry at all — refused as a
//! `4xx` before anything is published, rather than published and dropped by
//! every peer.

use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_account::{GovernanceOpKind, GovernanceWarrant};
use calimero_context_client::group::DelegatedGovernanceOp;
use calimero_context_client::local_governance::{GroupOp, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_server_primitives::admin::{
    GovernanceIntentApiRequest, GovernanceIntentApiResponse, GovernanceIntentApiResponseData,
    GovernanceIntentRelayApiResponse, GovernanceIntentRelayApiResponseData,
};
use eyre::WrapErr as _;
use reqwest::StatusCode;
use tracing::{debug, error, warn};

use crate::admin::handlers::context::create_context_intent::{internal, parse_group_id};
use crate::admin::handlers::context::perform_intent::{
    decode_author_proof, now_secs, IntentRefusal,
};
use crate::admin::handlers::identity::get_node_identity::node_identity;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

fn malformed(what: impl core::fmt::Display) -> eyre::Report {
    eyre::eyre!(IntentRefusal::Malformed(what.to_string()))
}

/// Decode the op the warrant covers, by the plane the warrant names, and check
/// it is exactly what was signed and one a relay may carry.
pub(crate) fn decode_covered_op(
    warrant: &GovernanceWarrant,
    group_id: &ContextGroupId,
    op_bytes: &[u8],
    now: u64,
) -> eyre::Result<DelegatedGovernanceOp> {
    if warrant.scope != group_id.to_bytes() {
        eyre::bail!(IntentRefusal::NotAuthorized(
            "this governance warrant is for a different group than the one it was presented in"
                .to_owned()
        ));
    }
    let (op, form) = match warrant.kind {
        GovernanceOpKind::Group => {
            let op: GroupOp = borsh::from_slice(op_bytes)
                .map_err(|err| malformed(format!("op is not a valid group op: {err}")))?;
            let form = op.delegable_form().ok_or_else(|| {
                eyre::eyre!(IntentRefusal::NotAuthorized(format!(
                    "the {} op cannot be published on a member's behalf",
                    op.op_kind_label()
                )))
            })?;
            let form = borsh::to_vec(&form)?;
            (
                DelegatedGovernanceOp::Group {
                    group_id: *group_id,
                    op,
                },
                form,
            )
        }
        GovernanceOpKind::Root => {
            let op: RootOp = borsh::from_slice(op_bytes)
                .map_err(|err| malformed(format!("op is not a valid root op: {err}")))?;
            let form = op.delegable_form().ok_or_else(|| {
                eyre::eyre!(IntentRefusal::NotAuthorized(
                    "this root op cannot be published on a member's behalf".to_owned()
                ))
            })?;
            let form = borsh::to_vec(&form)?;
            (DelegatedGovernanceOp::Root { op }, form)
        }
    };
    // The client sends the delegable form it signed; anything else is a
    // different op, whatever it hashes to.
    if form != op_bytes || !warrant.covers_op(warrant.kind, &form) {
        eyre::bail!(IntentRefusal::NotAuthorized(
            "this governance warrant does not cover the op presented with it".to_owned()
        ));
    }
    if warrant.not_after < now {
        eyre::bail!(IntentRefusal::NotAuthorized(format!(
            "this governance warrant expired at {} and it is now {now}; mint a fresh one",
            warrant.not_after
        )));
    }
    Ok(op)
}

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<GovernanceIntentApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    match perform(&state, group_id, req).await {
        Ok(data) => ApiResponse {
            payload: GovernanceIntentApiResponse { data },
        }
        .into_response(),
        Err(err) => {
            warn!(group_id = %group_id_str, %err, "refusing governance intent");
            parse_api_error(err).into_response()
        }
    }
}

async fn perform(
    state: &AdminState,
    group_id: ContextGroupId,
    req: GovernanceIntentApiRequest,
) -> eyre::Result<GovernanceIntentApiResponseData> {
    let warrant_bytes = hex::decode(req.warrant.trim())
        .map_err(|err| malformed(format!("warrant is not hex: {err}")))?;
    let warrant: GovernanceWarrant = borsh::from_slice(&warrant_bytes)
        .map_err(|err| malformed(format!("warrant is not a valid governance warrant: {err}")))?;
    let author_proof = decode_author_proof(&req.author_proof)?;
    let op_bytes =
        hex::decode(req.op.trim()).map_err(|err| malformed(format!("op is not hex: {err}")))?;

    let op = decode_covered_op(&warrant, &group_id, &op_bytes, now_secs())?;

    let store = state.ctx_client.datastore();
    refuse_unless_authentic_for_this_node(store, &warrant, &author_proof)?;
    let founding = matches!(
        &op,
        DelegatedGovernanceOp::Root {
            op: RootOp::NamespaceCreatedV2 { .. }
        }
    );
    // Founding a namespace: this node has no identity in it yet, so it takes
    // one now — the key its executor credential names and the genesis is
    // published with.
    let (signer, _secret) = if let DelegatedGovernanceOp::Root {
        op: RootOp::NamespaceCreatedV2 { founder, salt, .. },
    } = &op
    {
        refuse_unless_founded_by_author(&group_id, &warrant, founder, salt)?;
        let (_ns, pk, sk) =
            calimero_governance_store::NamespaceRepository::new(store).participate_in(&group_id)?;
        (pk, sk)
    } else {
        calimero_governance_store::NamespaceRepository::new(store)
            .resolve_identity(&group_id)?
            .ok_or_else(|| calimero_context::error::ContextError::NotAGroupMember {
                group_id: group_id.to_string(),
            })?
    };
    let op = match op {
        DelegatedGovernanceOp::Group {
            group_id,
            op:
                GroupOp::TargetApplicationSet {
                    target_application_id,
                    package,
                    version,
                    ..
                },
        } => {
            // Every peer refuses a delegated choice on a group that already has
            // one; say so now, before anything is fetched or published.
            calimero_governance_store::first_target_gate::refuse_unless_untargeted(
                store, &group_id,
            )?;
            let bytecode_id =
                resolve_bundle(state, &target_application_id, &package, &version).await?;
            DelegatedGovernanceOp::Group {
                group_id,
                op: GroupOp::TargetApplicationSet {
                    bytecode_id,
                    target_application_id,
                    package,
                    version,
                },
            }
        }
        other => other,
    };
    let executor_proof = calimero_context::join_credential::build(store, &group_id, &signer)
        .wrap_err("this node could not present its own credential")?;
    let delegation = calimero_account::GovernanceDelegation {
        warrant: Box::new(warrant),
        author_proof: Box::new(author_proof),
        executor_proof,
        executor_key: signer,
    };
    let verified = delegation
        .verify()
        .map_err(|err| malformed(format!("governance delegation does not verify: {err}")))?;
    debug!(%group_id, author = %verified.author_account, nonce = verified.nonce, "publishing a governance op on a member's behalf");

    let response = state
        .ctx_client
        .govern_on_behalf(calimero_context_client::group::GovernOnBehalfRequest { delegation, op })
        .await?;
    let (tee_enabled, tee_error) = if founding {
        match super::founding_attestation::attest(state, &group_id, signer).await {
            Ok(enabled) => (Some(enabled), None),
            Err(err) => {
                warn!(%group_id, error = %err, "founded the namespace, but could not admit this relay as its first TEE");
                (Some(false), Some(err))
            }
        }
    } else {
        (None, None)
    };
    Ok(GovernanceIntentApiResponseData {
        group_id: hex::encode(response.group_id.to_bytes()),
        tee_enabled,
        tee_error,
    })
}

/// Refuses a founding whose namespace id is not the warrant author's own, so a
/// relay never enlists in a namespace someone else founded.
fn refuse_unless_founded_by_author(
    group_id: &ContextGroupId,
    warrant: &GovernanceWarrant,
    founder: &calimero_primitives::identity::AccountId,
    salt: &[u8; calimero_account::NAMESPACE_SALT_LEN],
) -> eyre::Result<()> {
    if *founder != warrant.author_account
        || !calimero_account::is_founded_by(&group_id.to_bytes(), founder, salt)
    {
        eyre::bail!(IntentRefusal::NotAuthorized(
            "this namespace id is not one the warrant's author founds".to_owned()
        ));
    }
    Ok(())
}

/// Refuses a warrant whose author half does not verify, or that names another
/// executor, before anything is installed or recorded for it.
fn refuse_unless_authentic_for_this_node(
    store: &calimero_store::Store,
    warrant: &GovernanceWarrant,
    author_proof: &calimero_account::AccountProof<calimero_account::DeviceCert>,
) -> eyre::Result<()> {
    calimero_account::GovernanceDelegation::verify_author(warrant, author_proof)
        .map_err(|err| malformed(format!("governance delegation does not verify: {err}")))?;
    let executor = node_identity(store)?.map(|(account, ..)| account);
    if executor != Some(warrant.executor) {
        eyre::bail!(IntentRefusal::NotAuthorized(
            "this governance warrant names an executor other than this node's account".to_owned()
        ));
    }
    Ok(())
}

/// The `bytecode_id` a member's first `TargetApplicationSet` leaves for the relay
/// to fill: the blob id of the bundle this node installs for `package@version`.
///
/// The member cannot supply it — it is the id of the `.mpk` as stored here,
/// which no registry API publishes — and it claims nothing the member did not
/// sign: the bundle must be exactly `package@version`, and its manifest must
/// derive `application_id`, or the request is refused.
async fn resolve_bundle(
    state: &AdminState,
    application_id: &calimero_primitives::application::ApplicationId,
    package: &str,
    version: &str,
) -> eyre::Result<calimero_context_config::types::BytecodeId> {
    let node = &state.node_client;
    let installed = node.get_application(application_id)?.filter(|app| {
        app.size != 0
            && app.package == package
            && app.version.as_ref().map(ToString::to_string).as_deref() == Some(version)
    });
    let application = match installed {
        Some(app) if node.has_application(application_id)? => app,
        _ => {
            let Some(resolved) = node.install_by_coords(package, version).await? else {
                eyre::bail!(ApiError {
                    status_code: StatusCode::BAD_GATEWAY,
                    message: format!(
                        "this relay could not resolve {package}@{version} from its registry"
                    ),
                });
            };
            if resolved != *application_id {
                eyre::bail!(IntentRefusal::NotAuthorized(format!(
                    "{package}@{version} is application {resolved}, not the {application_id} \
                     the warrant pins"
                )));
            }
            node.get_application(application_id)?.ok_or_else(|| {
                eyre::eyre!("application {application_id} vanished after it was installed")
            })?
        }
    };
    Ok(calimero_context_config::types::BytecodeId::from(
        *application.blob.bytecode.digest(),
    ))
}

/// `GET` — the executor account to name, and whether this node may act for
/// members in the group at all. Signs nothing, spends nothing.
pub async fn describe_handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
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
                message: "this node holds neither a usable device nor an account root yet"
                    .to_owned(),
            }
            .into_response()
        }
        Err(err) => {
            error!(error = ?err, "Failed to read this node's identity");
            return internal("Failed to read this node's identity");
        }
    };
    let can_act_on_behalf =
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
    ApiResponse {
        payload: GovernanceIntentRelayApiResponse {
            data: GovernanceIntentRelayApiResponseData {
                executor_account: hex::encode(executor_account.as_bytes()),
                group_id: hex::encode(group_id.to_bytes()),
                can_act_on_behalf,
            },
        },
    }
    .into_response()
}

#[cfg(test)]
mod tests {
    use calimero_account::{GovernanceOpKind, GovernanceTerms, GovernanceWarrant};
    use calimero_context_client::group::DelegatedGovernanceOp;
    use calimero_context_client::local_governance::{GroupOp, RootOp};
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::{AccountId, PrivateKey};

    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use calimero_account::{AccountGenesis, AccountProof, DeviceCert, DeviceId, KemPublicKey};
    use calimero_context_client::client::ContextClient;
    use calimero_context_config::types::BytecodeId;
    use calimero_governance_store::{NamespaceRepository, NodeDeviceRepository};
    use calimero_primitives::application::ApplicationId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use libp2p::identity::Keypair;
    use tower::ServiceExt;

    use super::{decode_covered_op, node_identity};
    use crate::admin::handlers::context::perform_intent::IntentRefusal;

    const GROUP: [u8; 32] = [0x11; 32];
    const NOW: u64 = 1_700_000_000;
    const SALT: [u8; 32] = [0x5A; 32];
    const PUBLISH_WAIT: Duration = Duration::from_secs(2); // longer than any refusal takes

    fn warrant(kind: GovernanceOpKind, form: &[u8], not_after: u64) -> GovernanceWarrant {
        GovernanceWarrant::sign(
            &PrivateKey::from([7u8; 32]),
            GovernanceTerms {
                scope: GROUP,
                kind,
                author_account: AccountId::from([0x22; 32]),
                executor: AccountId::from([0x33; 32]),
                op_hash: GovernanceWarrant::op_hash(kind, form),
                account_heads: vec![],
                governance_floor: vec![],
                nonce: 1,
                not_after,
            },
        )
        .expect("sign")
    }

    fn add() -> GroupOp {
        GroupOp::MemberAdded {
            member: AccountId::from([0x44; 32]),
            role: GroupMemberRole::Member,
        }
    }

    fn not_authorized(err: &eyre::Report) -> String {
        match err.downcast_ref::<IntentRefusal>() {
            Some(IntentRefusal::NotAuthorized(msg)) => msg.clone(),
            other => panic!("expected NotAuthorized, got {other:?}: {err}"),
        }
    }

    #[test]
    fn the_signed_group_op_is_decoded_for_its_group() {
        let bytes = borsh::to_vec(&add()).expect("encode");
        let w = warrant(GovernanceOpKind::Group, &bytes, NOW + 60);
        let op = decode_covered_op(&w, &ContextGroupId::from(GROUP), &bytes, NOW).expect("covered");
        assert!(matches!(
            op,
            DelegatedGovernanceOp::Group {
                op: GroupOp::MemberAdded { .. },
                ..
            }
        ));
    }

    #[test]
    fn the_signed_root_op_is_decoded_on_the_root_plane() {
        let op = RootOp::GroupCreated {
            group_id: [0x55; 32].into(),
            parent_id: GROUP.into(),
            restricted: true,
            admin: AccountId::from([0x22; 32]),
            salt: [0; 32],
        };
        let bytes = borsh::to_vec(&op).expect("encode");
        let w = warrant(GovernanceOpKind::Root, &bytes, NOW + 60);
        let op = decode_covered_op(&w, &ContextGroupId::from(GROUP), &bytes, NOW).expect("covered");
        assert!(matches!(
            op,
            DelegatedGovernanceOp::Root {
                op: RootOp::GroupCreated { .. }
            }
        ));
    }

    #[test]
    fn another_op_than_the_signed_one_is_refused() {
        let bytes = borsh::to_vec(&add()).expect("encode");
        let w = warrant(GovernanceOpKind::Group, &bytes, NOW + 60);
        let other = borsh::to_vec(&GroupOp::MemberAdded {
            member: AccountId::from([0x45; 32]),
            role: GroupMemberRole::Member,
        })
        .expect("encode");
        let err = decode_covered_op(&w, &ContextGroupId::from(GROUP), &other, NOW).expect_err("x");
        assert!(not_authorized(&err).contains("does not cover"));
    }

    /// A removal must be sent in the form the member signs — hashes cleared —
    /// not with hashes filled in; those are the relay's to compute.
    #[test]
    fn an_op_not_in_its_delegable_form_is_refused() {
        let filled = GroupOp::MemberRemoved {
            member: AccountId::from([0x44; 32]),
            expected_group_state_hash: [0x42; 32],
            expected_context_state_hashes: vec![],
        };
        let bytes = borsh::to_vec(&filled).expect("encode");
        let w = warrant(GovernanceOpKind::Group, &bytes, NOW + 60);
        let err = decode_covered_op(&w, &ContextGroupId::from(GROUP), &bytes, NOW).expect_err("x");
        assert!(not_authorized(&err).contains("does not cover"));
    }

    /// A first application choice is signed with `bytecode_id` cleared: the
    /// member cannot know the blob id of the bundle this relay installs. Sent
    /// with one filled in, it is not the op the member signed.
    #[test]
    fn a_target_is_sent_with_the_bytecode_left_for_the_relay() {
        let target = |bytecode: [u8; 32]| GroupOp::TargetApplicationSet {
            bytecode_id: BytecodeId::from(bytecode),
            target_application_id: ApplicationId::from([0x88; 32]),
            package: "com.example.app".to_owned(),
            version: "1.2.3".to_owned(),
        };
        let form = borsh::to_vec(&target([0; 32])).expect("encode");
        let w = warrant(GovernanceOpKind::Group, &form, NOW + 60);
        let op = decode_covered_op(&w, &ContextGroupId::from(GROUP), &form, NOW).expect("covered");
        assert!(matches!(
            op,
            DelegatedGovernanceOp::Group {
                op: GroupOp::TargetApplicationSet { .. },
                ..
            }
        ));

        let filled = borsh::to_vec(&target([0x77; 32])).expect("encode");
        let w = warrant(GovernanceOpKind::Group, &filled, NOW + 60);
        let err = decode_covered_op(&w, &ContextGroupId::from(GROUP), &filled, NOW).expect_err("x");
        assert!(not_authorized(&err).contains("does not cover"));
    }

    #[test]
    fn a_non_delegable_op_is_refused_before_anything_is_published() {
        let op = GroupOp::TransferOwnership {
            new_owner: AccountId::from([0x44; 32]),
        };
        let bytes = borsh::to_vec(&op).expect("encode");
        let w = warrant(GovernanceOpKind::Group, &bytes, NOW + 60);
        let err = decode_covered_op(&w, &ContextGroupId::from(GROUP), &bytes, NOW).expect_err("x");
        assert!(not_authorized(&err).contains("cannot be published"));

        let root = RootOp::AdminChanged {
            new_admin: AccountId::from([0x44; 32]),
        };
        let bytes = borsh::to_vec(&root).expect("encode");
        let w = warrant(GovernanceOpKind::Root, &bytes, NOW + 60);
        let err = decode_covered_op(&w, &ContextGroupId::from(GROUP), &bytes, NOW).expect_err("x");
        assert!(not_authorized(&err).contains("cannot be published"));

        // Nor in their root-guarded form: a relay is exactly the party the root
        // guard keeps out, and it carries no proof of its own to add.
        let root_sk = calimero_primitives::identity::PrivateKey::from([0x21; 32]);
        let genesis = calimero_account::AccountGenesis::new(root_sk.public_key());
        let proof = calimero_account::SignedOwnerOp {
            genesis,
            chain: vec![],
            statement: calimero_account::OwnerOpAuthorization::sign(
                &root_sk,
                calimero_account::OwnerOpTerms {
                    account: genesis.account_id(),
                    namespace_id: GROUP,
                    group_id: GROUP,
                    kind: calimero_account::OwnerOpKind::TransferOwnership,
                    op_digest: [0; 32],
                    counter: 0,
                    key_epoch: 0,
                },
            )
            .expect("sign"),
        };
        let guarded = GroupOp::RootGuarded {
            op: Box::new(GroupOp::TransferOwnership {
                new_owner: AccountId::from([0x44; 32]),
            }),
            proof: Box::new(proof.clone()),
        };
        let bytes = borsh::to_vec(&guarded).expect("encode");
        let w = warrant(GovernanceOpKind::Group, &bytes, NOW + 60);
        let err = decode_covered_op(&w, &ContextGroupId::from(GROUP), &bytes, NOW).expect_err("x");
        assert!(not_authorized(&err).contains("cannot be published"));

        let guarded = RootOp::RootGuarded {
            op: Box::new(RootOp::AdminChanged {
                new_admin: AccountId::from([0x44; 32]),
            }),
            proof: Box::new(proof),
        };
        let bytes = borsh::to_vec(&guarded).expect("encode");
        let w = warrant(GovernanceOpKind::Root, &bytes, NOW + 60);
        let err = decode_covered_op(&w, &ContextGroupId::from(GROUP), &bytes, NOW).expect_err("x");
        assert!(not_authorized(&err).contains("cannot be published"));
    }

    #[test]
    fn expiry_group_and_encoding_are_checked() {
        let bytes = borsh::to_vec(&add()).expect("encode");
        let expired = warrant(GovernanceOpKind::Group, &bytes, NOW - 1);
        let err = decode_covered_op(&expired, &ContextGroupId::from(GROUP), &bytes, NOW)
            .expect_err("expired");
        assert!(not_authorized(&err).contains("expired"));

        let live = warrant(GovernanceOpKind::Group, &bytes, NOW + 60);
        let err = decode_covered_op(&live, &ContextGroupId::from([0x99; 32]), &bytes, NOW)
            .expect_err("other group");
        assert!(not_authorized(&err).contains("different group"));

        let err = decode_covered_op(&live, &ContextGroupId::from(GROUP), b"\xff\xff", NOW)
            .expect_err("garbage");
        assert!(matches!(
            err.downcast_ref::<IntentRefusal>(),
            Some(IntentRefusal::Malformed(_))
        ));
    }

    /// Group op bytes presented under a Root warrant decode as a root op (or
    /// fail to), never as the group op the member might have meant.
    #[test]
    fn the_plane_comes_from_the_warrant_not_the_bytes() {
        let bytes = borsh::to_vec(&add()).expect("encode");
        let root_warrant = warrant(GovernanceOpKind::Root, &bytes, NOW + 60);
        let result = decode_covered_op(&root_warrant, &ContextGroupId::from(GROUP), &bytes, NOW);
        assert!(!matches!(result, Ok(DelegatedGovernanceOp::Group { .. })));
    }

    /// The admin API's unauthenticated router as a relay serves it
    /// (`delegated_access`), over `store` with this node's identity provisioned.
    async fn public_router(store: &Store) -> (Router, tempfile::TempDir) {
        NodeDeviceRepository::new(store)
            .provision_account_root()
            .expect("this node's account root");
        let (event_sender, _rx) = tokio::sync::broadcast::channel(16);
        let (node_client, blob_dir) = crate::test_support::test_node_client(
            store,
            crate::test_support::stub_node_manager(vec![]),
            event_sender,
        )
        .await;
        let ctx_client =
            ContextClient::new(store.clone(), node_client.clone(), LazyRecipient::new());
        let state = Arc::new(crate::AdminState::new(
            store.clone(),
            ctx_client,
            node_client,
            Arc::new(crate::NodeReadiness::new()),
            [0; 32],
            #[cfg(feature = "mock-attestation")]
            false,
        ));
        let config = crate::config::ServerConfig::new(
            vec![],
            Keypair::generate_ed25519(),
            Some(crate::admin::service::AdminConfig::new(true, true)),
            None,
            None,
            None,
        );
        let (_path, _protected, public) =
            crate::admin::service::setup(&config, state).expect("admin api enabled");
        (public, blob_dir)
    }

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// The account this node relays as, the one a warrant must name.
    fn this_node(store: &Store) -> AccountId {
        node_identity(store)
            .expect("read identity")
            .expect("provisioned")
            .0
    }

    /// The author's device key, certified under the account rooted at seed 1.
    fn author() -> (PrivateKey, AccountProof<DeviceCert>) {
        let (root, device) = (PrivateKey::from([1u8; 32]), PrivateKey::from([7u8; 32]));
        let genesis = AccountGenesis::new(root.public_key());
        let account = genesis.account_id();
        let statement = DeviceCert::sign(
            &root,
            account,
            DeviceId::mint(account, [0x22; 16]),
            &device.public_key(),
            &KemPublicKey::from([9u8; 32]),
            0,
            0,
        )
        .expect("cert");
        let proof = AccountProof {
            statement,
            genesis,
            chain: Vec::new(),
        };
        (device, proof)
    }

    /// A warrant the author genuinely signed over `op` in `scope`, for `executor`.
    fn signed(
        scope: [u8; 32],
        executor: AccountId,
        kind: GovernanceOpKind,
        op: &[u8],
    ) -> GovernanceWarrant {
        let (device, proof) = author();
        GovernanceWarrant::sign(
            &device,
            GovernanceTerms {
                scope,
                kind,
                author_account: proof.genesis.account_id(),
                executor,
                op_hash: GovernanceWarrant::op_hash(kind, op),
                account_heads: vec![],
                governance_floor: vec![],
                nonce: 1,
                not_after: u64::MAX,
            },
        )
        .expect("sign")
    }

    /// The status `POST`ing `op` answers, or `None` if it is still waiting on
    /// the context actor this harness does not run, which only an op past every
    /// check reaches.
    async fn post(
        router: Router,
        group: [u8; 32],
        warrant: &GovernanceWarrant,
        op: &[u8],
    ) -> Option<StatusCode> {
        let body = serde_json::json!({
            "warrant": hex::encode(borsh::to_vec(warrant).expect("encode")),
            "authorProof": hex::encode(borsh::to_vec(&author().1).expect("encode")),
            "op": hex::encode(op),
        });
        let request = Request::builder()
            .method("POST")
            .uri(format!("/groups/{}/governance-intents", hex::encode(group)))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request");
        tokio::time::timeout(PUBLISH_WAIT, router.oneshot(request))
            .await
            .ok()
            .map(|response| response.expect("response").status())
    }

    /// The author founding a namespace with `SALT`.
    fn founding_op() -> Vec<u8> {
        let (_device, proof) = author();
        borsh::to_vec(&RootOp::NamespaceCreatedV2 {
            founder: proof.genesis.account_id(),
            account: Box::new(proof),
            salt: SALT,
        })
        .expect("encode")
    }

    /// The namespace `founding_op` founds.
    fn founded() -> [u8; 32] {
        calimero_account::founded_namespace_id(&author().1.genesis.account_id(), &SALT)
    }

    fn target_op() -> Vec<u8> {
        borsh::to_vec(&GroupOp::TargetApplicationSet {
            bytecode_id: BytecodeId::from([0; 32]),
            target_application_id: ApplicationId::from([0x88; 32]),
            package: "com.example.app".to_owned(),
            version: "1.2.3".to_owned(),
        })
        .expect("encode")
    }

    fn participates(store: &Store, group: [u8; 32]) -> bool {
        NamespaceRepository::new(store)
            .participating_namespaces()
            .expect("read participation")
            .contains(&ContextGroupId::from(group))
    }

    /// An unsigned warrant founding a namespace is refused before this node
    /// enlists in it.
    #[actix::test]
    async fn an_unsigned_founding_warrant_leaves_no_participation_behind() {
        let store = store();
        let (router, _blobs) = public_router(&store).await;
        let mut forged = signed(
            founded(),
            this_node(&store),
            GovernanceOpKind::Root,
            &founding_op(),
        );
        forged.signature = [0; 64];

        let status = post(router, founded(), &forged, &founding_op()).await;

        assert!(
            !participates(&store, founded()),
            "the refused warrant enlisted this node"
        );
        assert_eq!(status, Some(StatusCode::BAD_REQUEST));
    }

    /// A genuine warrant for another executor is refused before this node
    /// enlists, though only the executor half is wrong.
    #[actix::test]
    async fn a_warrant_for_another_executor_leaves_no_participation_behind() {
        let store = store();
        let (router, _blobs) = public_router(&store).await;
        let other = AccountId::from([0x33; 32]);
        let warrant = signed(founded(), other, GovernanceOpKind::Root, &founding_op());

        let status = post(router, founded(), &warrant, &founding_op()).await;

        assert!(
            !participates(&store, founded()),
            "the refused warrant enlisted this node"
        );
        assert_eq!(status, Some(StatusCode::FORBIDDEN));
    }

    /// A genuine warrant naming this node cannot enlist it in a namespace id its
    /// author did not found, such as one someone else founded.
    #[actix::test]
    async fn a_founding_of_another_namespace_id_leaves_no_participation_behind() {
        let store = store();
        let (router, _blobs) = public_router(&store).await;
        let warrant = signed(
            GROUP,
            this_node(&store),
            GovernanceOpKind::Root,
            &founding_op(),
        );

        let status = post(router, GROUP, &warrant, &founding_op()).await;

        assert!(
            !participates(&store, GROUP),
            "the refused warrant enlisted this node"
        );
        assert_eq!(status, Some(StatusCode::FORBIDDEN));
    }

    /// The control for the refusals above: a genuine founding of the author's own
    /// namespace, naming this node, gets past every check and enlists it.
    #[actix::test]
    async fn a_genuine_founding_for_this_node_gets_past_the_checks() {
        let store = store();
        let (router, _blobs) = public_router(&store).await;
        let warrant = signed(
            founded(),
            this_node(&store),
            GovernanceOpKind::Root,
            &founding_op(),
        );

        let status = post(router, founded(), &warrant, &founding_op()).await;

        assert_eq!(status, None, "a genuine founding was refused");
        assert!(
            participates(&store, founded()),
            "a genuine founding was refused"
        );
    }

    /// An unsigned warrant choosing a group's application is refused before
    /// this node fetches or installs anything for it.
    #[actix::test]
    async fn an_unsigned_target_warrant_is_refused_before_any_install() {
        let store = store();
        let (router, _blobs) = public_router(&store).await;
        let mut forged = signed(
            GROUP,
            this_node(&store),
            GovernanceOpKind::Group,
            &target_op(),
        );
        forged.signature = [0; 64];

        let status = post(router, GROUP, &forged, &target_op()).await;

        assert_eq!(
            status,
            Some(StatusCode::BAD_REQUEST),
            "refused as an unverifiable warrant, not after a registry fetch"
        );
    }

    /// A genuine target warrant for a group this node is not in is refused
    /// before anything is fetched or installed for it.
    #[actix::test]
    async fn a_target_for_a_group_this_node_is_not_in_is_refused_before_any_install() {
        let store = store();
        let (router, _blobs) = public_router(&store).await;
        let warrant = signed(
            GROUP,
            this_node(&store),
            GovernanceOpKind::Group,
            &target_op(),
        );

        let status = post(router, GROUP, &warrant, &target_op()).await;

        assert_eq!(
            status,
            Some(StatusCode::FORBIDDEN),
            "refused for membership, not after a registry fetch"
        );
    }
}
