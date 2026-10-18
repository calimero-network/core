//! `POST /admin-api/namespaces/:namespace_id/admit` — publish a join a joiner
//! signed but cannot publish itself.
//!
//! The joiner may hold no node at all. That is the case direct admission exists
//! for: an account with a key, a device certificate signed offline, and nowhere
//! to publish from.
//!
//! **The admitter relays; it does not author.** Every peer checks
//! `signer == credential.statement.sign_pk` when applying a join, so an admitter
//! that tried to sign on the joiner's behalf would produce an op the network
//! rejects. The op arrives already signed, and this node's contribution is to
//! decide whether to carry it — which is exactly the authority the invitation's
//! `admitters` list confers.
//!
//! What that buys: a hostile admitter can refuse to publish, and nothing else.
//! It cannot admit a different account, alter the group, or grant itself a role,
//! because all of that is inside a signature it does not hold.
//!
//! **Served without a credential, on every node** (`admission_routes` in
//! `admin/service.rs`). The caller holds nothing on this node a guard could
//! check, so the request is the whole authority, and the checks below run
//! cheapest first: a node the invitation does not name refuses before the op is
//! decoded, and nothing is signed or published until every check has passed.
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{
    AdmitJoinApiRequest, AdmitJoinApiResponse, AdmitJoinApiResponseData,
};
use reqwest::StatusCode;
use tracing::info;

use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_context_config::types::{ContextGroupId, SignedGroupOpenInvitation};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::handlers::identity::get_node_identity::node_identity;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<AdmitJoinApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    if req.invitation.invitation.group_id != namespace_id {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "invitation group_id does not match namespace_id in path".into(),
        }
        .into_response();
    }

    // Refuse before publishing, never after: once an op reaches the topic it is
    // on every peer's DAG, so "was this node entitled to carry it" has to be
    // answered while the answer still changes anything.
    //
    // And first, before the op is so much as decoded. The route takes no
    // credential, so whoever calls it chooses what it costs; a node the
    // invitation does not name answers with one store read and a list scan,
    // whatever bytes came with it. Running this ahead of the invitation's own
    // signature check is safe because it can only refuse: an `admitters` list
    // forged to name this node passes here and fails `validate_open_invitation`
    // below, before anything is signed or published.
    let self_account = match node_identity(&state.store) {
        Ok(Some((account, ..))) => account,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::CONFLICT,
                message: "this node holds no device, so it cannot admit anyone".to_owned(),
            }
            .into_response();
        }
        Err(err) => return parse_api_error(err).into_response(),
    };

    if let Err(err) = calimero_governance_store::NamespaceMembershipService::require_may_admit(
        &req.invitation,
        &self_account,
    ) {
        return ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: err.to_string(),
        }
        .into_response();
    }

    let signed_op_bytes = match hex::decode(&req.signed_op) {
        Ok(bytes) => bytes,
        Err(e) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: format!("signed_op is not hex: {e}"),
            }
            .into_response();
        }
    };

    // Decoded rather than relayed blind. Publishing opaque bytes would make a
    // designated admitter into a general-purpose injector for the namespace
    // topic — anything handed to it, signed by anyone, published under its own
    // connection. Decoding costs one borsh parse and bounds what this endpoint
    // can be used for.
    let mut op: SignedNamespaceOp = match borsh::from_slice(&signed_op_bytes) {
        Ok(op) => op,
        Err(e) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: format!("signed_op is not a SignedNamespaceOp: {e}"),
            }
            .into_response();
        }
    };

    if op.namespace_id.to_bytes() != namespace_id.to_bytes() {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "signed_op is for a different namespace than the path names".to_owned(),
        }
        .into_response();
    }

    let Some((member, claimed_invitation)) = join_claim(&op.op) else {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "signed_op is not a join; an admitter carries joins only".to_owned(),
        }
        .into_response();
    };
    let member = *member;

    // The bounds every peer applies to this op (parents, the invitation's
    // admitter and address lists), here rather than only at apply: an op over
    // them is refused by every replica, so carrying it is pure cost.
    if let Err(err) = op.validate() {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("signed_op is out of bounds: {err}"),
        }
        .into_response();
    }

    // The invitation checked below has to be the one the op claims. Otherwise
    // `validate_open_invitation` vouches for an invitation the op never names,
    // and anyone holding one invitation that lists this node could have it
    // carry joins under any other — an expired one, one for another group —
    // which every peer then refuses, after the op has travelled under this
    // node's name.
    if !same_signed_invitation(claimed_invitation, &req.invitation) {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "signed_op claims an invitation that is not the invitation presented"
                .to_owned(),
        }
        .into_response();
    }

    // The op's own signature, before anything else about it is trusted.
    //
    // Publishing puts this on the namespace topic under this node's connection.
    // Without this check a designated admitter forwards any well-formed join
    // whose signature is worthless — every peer rejects it at apply, so nothing
    // is admitted, but the garbage still travels and it travels with this node's
    // name on it. Decoding the borsh proved the shape, not the authorship.
    //
    // This is not the check that stops an admitter substituting a member: that is
    // `signer == credential.statement.sign_pk`, enforced at apply by every peer,
    // and it stays there because it must hold for ops this endpoint never saw.
    if let Err(err) = op.verify_signature() {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("signed_op signature does not verify: {err}"),
        }
        .into_response();
    }

    // Being a designated admitter is permission to carry a *valid* claim, not
    // permission to skip checking it: inviter signature, the group belonging to
    // this namespace, the inviter's permission to invite, and expiry.
    if let Err(err) = calimero_governance_store::NamespaceMembershipService::new(
        &state.store,
        namespace_id.to_bytes().into(),
    )
    .validate_open_invitation(&req.invitation, calimero_governance_store::now_secs())
    {
        return ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: format!("invitation rejected: {err}"),
        }
        .into_response();
    }

    // A join this node has already applied is not carried again.
    //
    // Each relay is a fresh envelope with its own id, so a replay of the same
    // signed op would put a new op on every peer's DAG per request, folding
    // nothing — and the route takes no credential, so whoever holds one valid
    // body could do that without end. One relay is enough: the first one is in
    // this node's op log, and sync carries it to any peer that missed the
    // gossip. The same two halves the apply guards a relayed join with: the op
    // log for a join that arrived in the clear, the joiner's nonce window for
    // one that only ever arrived relayed.
    match join_already_applied(&state.store, &namespace_id, &op) {
        Ok(false) => {}
        Ok(true) => {
            info!(namespace_id=%namespace_id_str, "a join already applied here; not relayed again");
            return ApiResponse {
                payload: AdmitJoinApiResponse {
                    data: AdmitJoinApiResponseData { published: false },
                },
            }
            .into_response();
        }
        Err(err) => return parse_api_error(err).into_response(),
    }

    // This node's consent, attached to the op it is about to carry.
    //
    // The joiner could not have supplied this: it has no node, and an
    // endorsement can only be signed by an account the invitation named. So the
    // admitter adds its own as it relays — which is possible precisely because
    // the endorsement rides the envelope and is outside the joiner's signature.
    // Attaching it changes neither that signature nor the op's id.
    //
    // Overwritten rather than preserved if the caller sent one. Whatever a
    // keyholder put there, this node is the one vouching for the op it
    // publishes under its own connection, and `require_may_admit` above has
    // already established it may. A caller-supplied endorsement would have to
    // be re-verified to be worth keeping, and there is nothing to gain by
    // keeping it: any endorsement that verifies is over the same namespace,
    // joiner and invitation as the one written here.
    match calimero_governance_store::NamespaceMembershipService::endorse_join(
        &state.store,
        &namespace_id,
        &member,
        &req.invitation,
    ) {
        Ok(Some(endorsement)) => op.admitter_endorsement = Some(Box::new(endorsement)),
        // `require_may_admit` passed, so the invitation names an account —
        // reaching here means this node cannot resolve its own identity to
        // that account, i.e. it is named but holds nothing to sign with.
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::CONFLICT,
                message: "this node is named as an admitter but holds no namespace identity \
                          bound to that account, so it cannot endorse this join"
                    .to_owned(),
            }
            .into_response();
        }
        Err(err) => return parse_api_error(err).into_response(),
    }

    // Sealed and published as one step, by the actor that holds both keys.
    //
    // The joiner's op goes inside the seal exactly as it stands — endorsement
    // attached above and its own signature untouched — and this node signs the
    // envelope that carries it. Peers decrypt, verify the JOINER'S signature and
    // apply the join, so `signer == credential.statement.sign_pk` still decides
    // who joined and a hostile admitter still cannot admit a different account.
    //
    // What sealing removes is every non-member on the namespace topic being able
    // to read which account joined which group, and when.
    //
    // The relay applies locally as part of publishing, so there is no separate
    // apply here. It has to: publishing alone would leave the one node the joiner
    // actually talked to with the stalest view of the membership it just
    // admitted, and with no mesh peers nobody would fold it at all.
    if let Err(err) = state.ctx_client.relay_signed_join(op).await {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("join could not be relayed: {err}"),
        }
        .into_response();
    }

    info!(namespace_id=%namespace_id_str, "admitted a joiner's signed join op");

    ApiResponse {
        // The wrapper type, not the payload type: `ApiResponse` serialises what
        // it is given verbatim, so handing it the inner struct omits the `data`
        // key every client unwraps by — which reads as a successful call
        // returning nothing rather than as an error.
        payload: AdmitJoinApiResponse {
            data: AdmitJoinApiResponseData { published: true },
        },
    }
    .into_response()
}

/// The account a join admits and the invitation it claims, for either join
/// variant; `None` for anything an admitter does not carry.
///
/// Only a join. Being named in `admitters` is authority to admit somebody, not
/// to publish governance at large on their behalf — without this an admitter is
/// a general-purpose injector for the namespace topic, publishing whatever it is
/// handed under its own connection.
///
/// `MemberJoinedOpen` is deliberately absent: it carries no invitation, so there
/// is nothing naming this node as entitled to carry it.
const fn join_claim(
    op: &NamespaceOp,
) -> Option<(&calimero_account::AccountId, &SignedGroupOpenInvitation)> {
    match op {
        NamespaceOp::Root(
            RootOp::MemberJoined {
                member,
                signed_invitation,
                ..
            }
            | RootOp::MemberJoinedAt {
                member,
                signed_invitation,
                ..
            },
        ) => Some((member, signed_invitation)),
        _ => None,
    }
}

#[cfg(test)]
const fn carries_a_join(op: &NamespaceOp) -> bool {
    join_claim(op).is_some()
}

/// Whether two invitations are the same signed claim: the invitation the
/// inviter signed, and its signature. `admitter_addrs` and the other unsigned
/// hints may differ, since anyone relaying an invitation may rewrite them.
fn same_signed_invitation(a: &SignedGroupOpenInvitation, b: &SignedGroupOpenInvitation) -> bool {
    a.inviter_signature == b.inviter_signature
        && matches!(
            (borsh::to_vec(&a.invitation), borsh::to_vec(&b.invitation)),
            (Ok(a), Ok(b)) if a == b
        )
}

/// Whether `op` is a join this node has already applied, in the clear or
/// relayed. Mirrors the replay guard `NamespaceGovernance::apply_signed_op`
/// puts on a relayed join's inner op, read without recording anything.
fn join_already_applied(
    store: &calimero_store::Store,
    namespace_id: &ContextGroupId,
    op: &SignedNamespaceOp,
) -> eyre::Result<bool> {
    let id = op
        .content_hash()
        .map_err(|e| eyre::eyre!("signed_op content hash: {e}"))?;
    if calimero_governance_store::NamespaceOpLogService::new(store, namespace_id.to_bytes().into())
        .contains_op(id)?
    {
        return Ok(true);
    }
    Ok(
        calimero_governance_store::load_nonce_window(store, namespace_id, &op.signer)?
            .contains(op.nonce),
    )
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp};

    use super::carries_a_join;

    #[test]
    fn a_join_is_carried() {
        let genesis = calimero_account::AccountGenesis::new(
            calimero_primitives::identity::PublicKey::from([0u8; 32]),
        );
        let credential = calimero_context_client::local_governance::JoinAccountCredential {
            statement: calimero_account::DeviceCert {
                account: genesis.account_id(),
                device: calimero_account::DeviceId::from([0u8; 32]),
                sign_pk: calimero_primitives::identity::PublicKey::from([0u8; 32]),
                kem_pk: calimero_account::KemPublicKey::from([0u8; 32]),
                key_epoch: 0,
                device_epoch: 0,
                signature: [0u8; 64],
            },
            genesis,
            chain: Vec::new(),
        };
        let invitation = calimero_context_config::types::SignedGroupOpenInvitation {
            inviter_account: None,
            invitation: calimero_context_config::types::GroupInvitationFromAdmin {
                inviter_identity: calimero_context_config::types::SignerId::from([0u8; 32]),
                group_id: calimero_context_config::types::ContextGroupId::from([0u8; 32]),
                expiration_timestamp: 0,
                invitation_nonce: [0u8; 32],
                invited_role: 1,
                admitters: Vec::new(),
            },
            inviter_signature: String::new(),
            admitter_addrs: Vec::new(),
            application_id: None,
            bytecode_id: None,
        };

        assert!(carries_a_join(&NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_account::AccountId::from([0u8; 32]),
            signed_invitation: invitation,
            account: Box::new(credential),
        })));
    }

    /// Governance that is not a join must be refused.
    ///
    /// This is the rule that stops a designated admitter being used to publish
    /// arbitrary namespace governance signed by whoever asked.
    #[test]
    fn governance_that_is_not_a_join_is_refused() {
        let admin_changed = NamespaceOp::Root(RootOp::AdminChanged {
            new_admin: calimero_account::AccountId::from([0x11; 32]),
        });
        assert!(!carries_a_join(&admin_changed));

        let policy = NamespaceOp::Root(RootOp::PolicyUpdated {
            policy_bytes: Vec::new(),
        });
        assert!(!carries_a_join(&policy));
    }
}

/// The route as a joiner meets it: a node with embedded auth, assembled the way
/// `start` assembles it, and a caller holding no credential of any kind.
#[cfg(test)]
mod over_http {
    use std::sync::Arc;

    use axum::http::StatusCode;
    use axum::Router;
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_context_config::types::{
        ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use serde_json::{json, Value};
    use tempfile::TempDir;

    use crate::admin::handlers::identity::get_node_identity::node_identity;
    use crate::admin::service::AdminConfig;
    use crate::config::{AuthMode, ServerConfig};
    use crate::service_mounts::{mount_runtime_services, RuntimeServiceDeps};
    use crate::test_support::{admin_state, embedded_auth_service, post_json_uncredentialed};

    const NAMESPACE: [u8; 32] = [0x5a; 32];

    /// A node with embedded auth and the default posture: `delegated_access`
    /// off, so nothing about this route depends on the relay flag.
    async fn embedded_auth_node(store: &Store) -> (Router, TempDir) {
        let (state, blob_dir) = admin_state(store).await;
        let mut config = ServerConfig::new(
            vec![],
            libp2p::identity::Keypair::generate_ed25519(),
            Some(AdminConfig::new(true, false)),
            None,
            None,
            None,
        );
        config.auth_mode = AuthMode::Embedded;
        let deps = RuntimeServiceDeps {
            auth_service: Some(embedded_auth_service().await),
            ctx_client: state.ctx_client.clone(),
            node_client: state.node_client.clone(),
            datastore: store.clone(),
            shared_state: Arc::clone(&state),
            prom_registry: prometheus_client::registry::Registry::default(),
        };
        (
            mount_runtime_services(Router::new(), &config, deps).router,
            blob_dir,
        )
    }

    fn admit_uri() -> String {
        format!("/admin-api/namespaces/{}/admit", hex::encode(NAMESPACE))
    }

    /// An invitation to the namespace naming `admitters`. Its inviter signature
    /// is not a real one: every refusal below must come before it is checked.
    fn invitation(admitters: Vec<calimero_account::AccountId>) -> SignedGroupOpenInvitation {
        SignedGroupOpenInvitation {
            inviter_account: None,
            invitation: GroupInvitationFromAdmin {
                inviter_identity: SignerId::from([0x11; 32]),
                group_id: ContextGroupId::from(NAMESPACE),
                expiration_timestamp: 0,
                invitation_nonce: [0x22; 32],
                invited_role: 1,
                admitters,
            },
            inviter_signature: String::new(),
            admitter_addrs: Vec::new(),
            application_id: None,
            bytecode_id: None,
        }
    }

    fn body(invitation: &SignedGroupOpenInvitation, signed_op: &str) -> Value {
        json!({ "invitation": invitation, "signedOp": signed_op })
    }

    /// This node's own account, which an invitation must name for it to admit.
    fn this_node(store: &Store) -> calimero_account::AccountId {
        node_identity(store)
            .expect("read this node's identity")
            .expect("this node has an account root")
            .0
    }

    /// A joiner's signed join of the namespace under `invitation`.
    fn signed_join(
        joiner: &PrivateKey,
        invitation: SignedGroupOpenInvitation,
    ) -> SignedNamespaceOp {
        let account =
            calimero_governance_store::test_fixtures::real_join_account(&joiner.public_key());
        SignedNamespaceOp::sign(
            joiner,
            NAMESPACE.into(),
            vec![],
            1,
            NamespaceOp::Root(RootOp::MemberJoined {
                member: account.statement.account,
                signed_invitation: invitation,
                account,
            }),
        )
        .expect("the joiner signs its join")
    }

    /// A join this node already applied is not relayed again: each relay is a
    /// new envelope on every peer's DAG, and the route takes no credential.
    #[test]
    fn a_join_already_applied_is_recognised_by_its_nonce_or_its_id() {
        use calimero_governance_store::{load_nonce_window, store_nonce_window};

        use super::join_already_applied;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let namespace = ContextGroupId::from(NAMESPACE);
        let joiner = PrivateKey::from([0x44; 32]);
        let op = signed_join(&joiner, invitation(vec![]));

        assert!(!join_already_applied(&store, &namespace, &op).unwrap());

        let mut window = load_nonce_window(&store, &namespace, &op.signer).unwrap();
        assert!(window.record(op.nonce));
        store_nonce_window(&store, &namespace, &op.signer, &window).unwrap();
        assert!(
            join_already_applied(&store, &namespace, &op).unwrap(),
            "a relayed join leaves only the joiner's nonce behind"
        );
    }

    /// The bug: a joiner holds a device key and an invitation, nothing on this
    /// node, and every SDK posts with no `Authorization` and no proof. On a node
    /// with embedded auth the route sat behind the guard, so the joiner was
    /// answered `401` before the handler ever saw the signed op that is its
    /// authorization.
    #[actix::test]
    async fn an_uncredentialed_admit_reaches_the_handler_on_an_embedded_auth_node() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (router, _blob_dir) = embedded_auth_node(&store).await;

        // An invitation naming some other node: the handler's own refusal,
        // not the guard's.
        let elsewhere = calimero_account::AccountId::from([0x33; 32]);
        let (status, answer) = post_json_uncredentialed(
            router.clone(),
            &admit_uri(),
            &body(&invitation(vec![elsewhere]), "00"),
        )
        .await;
        assert_ne!(
            status,
            StatusCode::UNAUTHORIZED,
            "an admit carrying no credential was refused by the auth guard: {answer}"
        );
        assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
        assert!(answer.contains("not an admitter"), "{answer}");

        // Only this route left the guard: its namespace siblings still need one.
        let join = format!("/admin-api/namespaces/{}/join", hex::encode(NAMESPACE));
        let (status, _) = post_json_uncredentialed(router, &join, &json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// Without a credential the request is the authority, so everything the
    /// handler refuses must still be refused, and before anything is published.
    #[actix::test]
    async fn an_uncredentialed_admit_that_does_not_verify_is_refused() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (router, _blob_dir) = embedded_auth_node(&store).await;
        let named = invitation(vec![this_node(&store)]);

        let joiner = PrivateKey::from([0x44; 32]);
        let mut forged = signed_join(&joiner, named.clone());
        forged.signature[0] ^= 0xff;

        let mut other_invitation = named.clone();
        other_invitation.invitation.invitation_nonce = [0x99; 32];
        let under_another_invitation = signed_join(&joiner, other_invitation);

        for (case, signed_op, expect) in [
            ("not hex", "zz".to_owned(), "signed_op is not hex"),
            (
                "not a signed op",
                hex::encode([0xab; 16]),
                "signed_op is not a SignedNamespaceOp",
            ),
            (
                "a join whose signature does not verify",
                hex::encode(borsh::to_vec(&forged).unwrap()),
                "signature does not verify",
            ),
            (
                "a join carrying another invitation than the one presented",
                hex::encode(borsh::to_vec(&under_another_invitation).unwrap()),
                "not the invitation presented",
            ),
        ] {
            let (status, answer) =
                post_json_uncredentialed(router.clone(), &admit_uri(), &body(&named, &signed_op))
                    .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{case}: {answer}");
            assert!(answer.contains(expect), "{case}: {answer}");
        }

        // A well-formed join whose invitation's inviter signature is worthless.
        let unsigned = signed_join(&joiner, named.clone());
        let (status, answer) = post_json_uncredentialed(
            router,
            &admit_uri(),
            &body(&named, &hex::encode(borsh::to_vec(&unsigned).unwrap())),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
        assert!(answer.contains("invitation rejected"), "{answer}");
    }
}
