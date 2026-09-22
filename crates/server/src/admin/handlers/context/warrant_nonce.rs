//! `GET /admin-api/contexts/:context_id/warrant-nonce/:author_device_key` —
//! where an author device stands in its warrant-nonce sequence here.
//!
//! and `POST /admin-api/contexts/:context_id/warrant-nonce` — the same read for
//! a delegated author asking about **itself**, presenting the credential that
//! proves which device it is. See [`delegated_handler`].
//!
//! # The gap this closes
//!
//! `GET .../intents` already tells a keyholder everything about *this relay* it
//! needs before minting a warrant: the executor account to name, and whether the
//! relay may act at all. What it cannot tell it is the one input that is stateful
//! on the **client** — the nonce. An author that still remembers its own counter
//! does not need this route. An author that has lost it has, without this route,
//! exactly one strategy: guess upward and spend a refused round trip per wrong
//! guess, unbounded, with each refusal indistinguishable at the API from a
//! genuinely spent nonce.
//!
//! That is not a corner. A browser keyholder lives in storage that is
//! partitioned per top-level site and cleared on a schedule it does not control,
//! so losing the counter is its normal lifecycle rather than a fault. The nonce
//! being unrecoverable makes the ledger's replay protection read, from the
//! client's side, as data loss.
//!
//! # Why an authenticated admin read rather than a public one
//!
//! The obvious place for this is beside the two delegated-execution routes, which
//! move together for a reason stated there: a client that can `POST` but not
//! `GET` cannot learn what to put in the warrant. By that argument this belongs
//! with them.
//!
//! It is mounted on the protected router anyway, and the difference is who may
//! ask about *whom*. Those two routes are about the relay itself — the executor
//! account it publishes, the grant it holds — and answer the same for every
//! caller. This one takes another principal's device key as input and reports
//! that principal's activity. The number is not a secret (see
//! `calimero_governance_store::warrant_gate::warrant_nonce_state`: every peer
//! replicating the context folds it out of the log), but "not secret to the
//! members" and "answerable to an anonymous caller" are different postures, and
//! choosing the narrower one costs a public-intents deployment a route it can
//! re-mount deliberately, while the reverse costs a decision nobody made.
//!
//! That posture held, and cost the feature its only client: a delegated author
//! holds a session, not node credentials, so the recovery path was unreachable
//! from the deployment it was built for. The `POST` form resolves it without
//! widening the `GET`: asking about *whom* stops being a question the route has
//! to take on trust, because the caller proves the device is its own with a
//! root-signed certificate. Both refusals below are one message for that reason.

use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_account::{AccountId, AccountProof, DeviceCert};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    WarrantNonceApiRequest, WarrantNonceApiResponse, WarrantNonceApiResponseData,
};
use calimero_store::types::ContextWarrantNonce;
use reqwest::StatusCode;
use tracing::{error, warn};

use crate::admin::service::{ApiError, ApiResponse};
use crate::auth::AuthenticatedAccount;
use crate::AdminState;

/// Parse the device key from the path, accepting both spellings it has on this
/// node's own surfaces.
///
/// `GET /admin-api/account/devices` renders a device's `signingKey` as base58,
/// which is how `PublicKey` serialises everywhere. The delegated-execution
/// surface next door is hex throughout — `executorAccount` is hex, and the
/// author's own key reaches a relay as hex bytes inside the borsh warrant. A
/// client legitimately holds the same key in either form depending on where it
/// got it, and a 404-shaped "unknown device" for the wrong spelling is the kind
/// of failure that gets diagnosed as a missing row.
///
/// Accepting both is unambiguous rather than lenient: 32 bytes is 43-44
/// characters of base58 and exactly 64 of hex, so the two encodings of a key
/// never collide, and anything that is neither is still refused.
fn parse_device_key(raw: &str) -> Option<PublicKey> {
    if let Ok(key) = raw.parse::<PublicKey>() {
        return Some(key);
    }
    let bytes = hex::decode(raw).ok()?;
    let bytes: [u8; 32] = bytes.try_into().ok()?;
    Some(PublicKey::from(bytes))
}

/// Shape the ledger row into the answer, which is the only place the
/// `high_water + 1` rule is applied.
///
/// Split out and pure so the rule a client is told can be tested without a
/// store: it is the entire contract of this endpoint, and a client that misreads
/// it burns nonces for real.
fn describe(
    context_id: ContextId,
    author_device_key: PublicKey,
    state: Option<ContextWarrantNonce>,
) -> WarrantNonceApiResponseData {
    WarrantNonceApiResponseData {
        context_id,
        author_device_key,
        seen: state.is_some(),
        high_water_nonce: state.map(|s| s.high_water),
        // Nothing strictly above the mark has been accepted, so one past it is
        // the smallest nonce this node is guaranteed to take. `checked_add`
        // rather than `+`: a device that has spent `u64::MAX` here has no next
        // nonce, and reporting a wrapped `0` would hand it a nonce certain to be
        // refused while looking exactly like a valid answer.
        next_nonce: state.map_or(Some(0), |s| s.high_water.checked_add(1)),
        window_width: ContextWarrantNonce::WINDOW,
    }
}

pub async fn handler(
    Path((context_id_str, device_key_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let context_id: ContextId = match context_id_str.parse() {
        Ok(id) => id,
        Err(err) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: format!("context id '{context_id_str}' is not valid: {err}"),
            }
            .into_response()
        }
    };

    let Some(author_device_key) = parse_device_key(&device_key_str) else {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "author device key must be a 32-byte public key, base58 or hex".to_owned(),
        }
        .into_response();
    };

    serve(&state, context_id, author_device_key)
}

/// Read the ledger and shape the answer. The single place either route produces
/// a `200`, so the two cannot drift: a client is told the same thing whether an
/// admin asked on its behalf or it asked for itself.
fn serve(
    state: &AdminState,
    context_id: ContextId,
    author_device_key: PublicKey,
) -> axum::response::Response {
    let store = state.ctx_client.datastore();

    // Deliberately NOT gated on the context belonging to a group, unlike
    // `GET .../intents`. That route reports whether a delegated write could be
    // authorized, so "no owning group" is a real answer to its question. This one
    // reports a counter, and the counter is the same whatever the group state is;
    // refusing here would withhold a recovery from a client whose context is
    // mid-registration and send it back to guessing.
    let nonce_state = match calimero_governance_store::warrant_gate::warrant_nonce_state(
        store,
        &context_id,
        author_device_key,
    ) {
        Ok(state) => state,
        Err(err) => {
            error!(error = ?err, %context_id, "Failed to read this device's warrant nonce state");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to read this device's warrant nonce state".to_owned(),
            }
            .into_response();
        }
    };

    ApiResponse {
        payload: WarrantNonceApiResponse {
            data: describe(context_id, author_device_key, nonce_state),
        },
    }
    .into_response()
}

/// The one refusal every bad credential gets.
///
/// Not-hex, not-borsh, and a perfectly-formed proof for somebody else's account
/// are one status and one message on purpose. Told apart, they answer a question
/// nobody asked the route: "is this credential real, and whose?". A caller that
/// could distinguish "this did not parse" from "this is valid but not yours"
/// could test a captured proof against the route to learn whether it verifies —
/// which is the oracle the admin-only placement exists to avoid, rebuilt in the
/// error channel.
///
/// `403` rather than `400`: the caller did present a credential, and the node
/// declined to act on it. Malformed input is folded up into that rather than the
/// other way round, because the indistinguishability has to hold in the
/// direction that leaks.
fn credential_refused() -> ApiError {
    ApiError {
        status_code: StatusCode::FORBIDDEN,
        message: "authorProof is not a valid credential for the authenticated account".to_owned(),
    }
}

/// Which device the caller has proved itself to be, or the one refusal.
///
/// Split out and free of both axum and the store so the rule can be tested
/// directly: this function is the entire security argument for the delegated
/// route, and it is three lines that are easy to get subtly wrong.
///
/// `account` comes from the auth layer, never from the request. A caller may
/// choose which account it authenticates as and nothing more.
fn authorised_device(account: AccountId, author_proof_hex: &str) -> Result<PublicKey, ApiError> {
    let bytes = hex::decode(author_proof_hex.trim()).map_err(|_| credential_refused())?;
    let proof: AccountProof<DeviceCert> =
        borsh::from_slice(&bytes).map_err(|_| credential_refused())?;

    // The whole check: the root that derives THIS account signed this
    // certificate. A proof for another account fails here, and it fails without
    // the node having to know anything about that other account.
    let cert = proof.verify(account).map_err(|_| credential_refused())?;

    // From the VERIFIED certificate, never from the request. `sign_pk` is the
    // key that signs the author's warrants, which is exactly what the nonce
    // ledger is keyed by — see `Warrant::author_device_key`, which a delegation
    // checks equals this field.
    Ok(cert.sign_pk)
}

/// `POST /admin-api/contexts/:context_id/warrant-nonce` — the same read, for a
/// delegated author asking about itself.
///
/// The admin route beside this one may ask about any device, and so requires
/// node credentials. That made the feature unreachable by the only client that
/// needs it: a delegated author holds a session scoped
/// `context:intent|query|subscribe`, and an operator handing it an admin
/// credential so it can recover a counter defeats the delegated surface.
///
/// The session cannot say which DEVICE is calling — `account_proof` subjects the
/// token to the account deliberately — so this route does not ask it to. The
/// caller presents the credential that proves device-to-account, which it must
/// already hold or it could not have written here at all, and the node serves
/// the device key out of the verified certificate. No new claim, no new provider
/// interface, and no dependence on the device having joined a group — which the
/// account-scoped alternative would have needed, and which a thin client's
/// signing key never does.
pub async fn delegated_handler(
    Path(context_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    account: Option<Extension<AuthenticatedAccount>>,
    Json(req): Json<WarrantNonceApiRequest>,
) -> impl IntoResponse {
    let context_id: ContextId = match context_id_str.parse() {
        Ok(id) => id,
        Err(err) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: format!("context id '{context_id_str}' is not valid: {err}"),
            }
            .into_response()
        }
    };

    // Absent means the session is not account-anchored — a node-owner or
    // client-key session. Refused not for want of permission but because this
    // route answers "where does the device THIS ACCOUNT proved stand", and there
    // is no account to check the proof against. A node owner uses the admin GET.
    let Some(Extension(AuthenticatedAccount(account))) = account else {
        return ApiError {
            status_code: StatusCode::UNAUTHORIZED,
            message: "this endpoint requires an account-authenticated session".to_owned(),
        }
        .into_response();
    };

    let author_device_key = match authorised_device(account, &req.author_proof) {
        Ok(key) => key,
        Err(refusal) => {
            warn!(%context_id, %account, "refusing a warrant-nonce read: credential did not verify");
            return refusal.into_response();
        }
    };

    serve(&state, context_id, author_device_key)
}

#[cfg(test)]
mod tests {
    use calimero_account::{AccountGenesis, AccountId, AccountProof, DeviceCert, KemPublicKey};
    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::{DeviceId, PrivateKey, PublicKey};
    use calimero_store::types::ContextWarrantNonce;

    use super::{authorised_device, describe, parse_device_key};

    const CONTEXT: [u8; 32] = [0xC1; 32];
    const DEVICE: [u8; 32] = [0x0A; 32];

    fn context() -> ContextId {
        ContextId::from(CONTEXT)
    }

    fn device() -> PublicKey {
        PublicKey::from(DEVICE)
    }

    /// The admin API is camelCase, and a snake_case field is not a rename — it is
    /// a field the client never sees, surfacing as a "missing field" for
    /// something the server believes it sent. Asserted on the literal JSON rather
    /// than through a round trip, because a round trip through the same struct
    /// agrees with itself whatever the casing is.
    #[test]
    fn the_response_is_camel_case_on_the_wire() {
        let json = serde_json::to_value(describe(
            context(),
            device(),
            Some(ContextWarrantNonce::first(7)),
        ))
        .expect("the response must serialise");
        let obj = json.as_object().expect("an object");

        for field in [
            "contextId",
            "authorDeviceKey",
            "seen",
            "highWaterNonce",
            "nextNonce",
            "windowWidth",
        ] {
            assert!(obj.contains_key(field), "missing `{field}` in {json}");
        }
        for snake in [
            "context_id",
            "author_device_key",
            "high_water_nonce",
            "next_nonce",
            "window_width",
        ] {
            assert!(
                !obj.contains_key(snake),
                "`{snake}` leaked in snake_case: {json}"
            );
        }
    }

    /// The contract, stated once: one past the mark.
    #[test]
    fn the_next_nonce_is_one_past_the_high_water_mark() {
        let data = describe(context(), device(), Some(ContextWarrantNonce::first(7)));

        assert!(data.seen);
        assert_eq!(data.high_water_nonce, Some(7));
        assert_eq!(data.next_nonce, Some(8));
        assert_eq!(data.window_width, ContextWarrantNonce::WINDOW);
    }

    /// The window below the mark is partly spent, and how it is spent must not
    /// change the answer a client acts on — it is told one rule.
    #[test]
    fn the_window_below_the_mark_does_not_move_the_next_nonce() {
        let ragged = ContextWarrantNonce {
            high_water: 7,
            window: 0b1010_1010,
        };

        assert_eq!(
            describe(context(), device(), Some(ragged)).next_nonce,
            Some(8)
        );
    }

    /// A device with no row must be told so explicitly, not handed a zero it
    /// cannot tell apart from "nonce 0 is spent".
    #[test]
    fn a_device_with_no_row_is_reported_as_unseen() {
        let data = describe(context(), device(), None);

        assert!(!data.seen);
        assert_eq!(data.high_water_nonce, None);
        assert_eq!(
            data.next_nonce,
            Some(0),
            "a fresh sequence starts at the bottom"
        );
    }

    /// `seen: false` must be visible in the JSON, since its two companions are
    /// omitted when absent and a client reading only those cannot distinguish a
    /// fresh device from a truncated response.
    #[test]
    fn an_unseen_device_still_reports_a_next_nonce() {
        let json = serde_json::to_value(describe(context(), device(), None))
            .expect("the response must serialise");
        let obj = json.as_object().expect("an object");

        assert_eq!(
            obj.get("seen").and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert!(
            !obj.contains_key("highWaterNonce"),
            "absent, not null: {json}"
        );
        assert_eq!(
            obj.get("nextNonce").and_then(serde_json::Value::as_u64),
            Some(0)
        );
    }

    /// The saturation case. There is no next nonce, and saying so is the only
    /// honest answer — a wrapped `0` would look valid and be refused forever.
    #[test]
    fn a_saturated_sequence_offers_no_next_nonce() {
        let data = describe(
            context(),
            device(),
            Some(ContextWarrantNonce::first(u64::MAX)),
        );

        assert_eq!(data.high_water_nonce, Some(u64::MAX));
        assert_eq!(
            data.next_nonce, None,
            "this device must re-key to keep writing"
        );
    }

    /// Both spellings a client can legitimately be holding resolve to one key.
    #[test]
    fn a_device_key_parses_from_either_encoding() {
        let expected = device();

        let base58 = parse_device_key(&expected.to_string()).expect("base58 must parse");
        let hexed = parse_device_key(&hex::encode(DEVICE)).expect("hex must parse");

        assert_eq!(base58, expected);
        assert_eq!(hexed, expected);
    }

    /// And nothing else does — accepting both encodings must not become
    /// accepting anything.
    #[test]
    fn a_malformed_device_key_is_refused() {
        for bad in ["", "not-a-key", &hex::encode([0x0A; 31]), &"0".repeat(65)] {
            assert!(
                parse_device_key(bad).is_none(),
                "`{bad}` must not parse as a device key"
            );
        }
    }

    // ---- the delegated route: which device has the caller proved itself to be ----

    /// An account and one of its devices, with the credential the device would
    /// send. Deterministic so a failure reproduces exactly.
    ///
    /// Built through the real minters rather than a fixture blob, so the test
    /// exercises the same verification a live `authorProof` goes through — this
    /// is the one check standing between a session and another device's counter.
    fn account_with_device(root_seed: u8, device_seed: u8) -> (AccountId, PublicKey, String) {
        let root = PrivateKey::from([root_seed; 32]);
        let device_sk = PrivateKey::from([device_seed; 32]);
        let genesis = AccountGenesis::new(root.public_key());
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [device_seed; 16]);

        let cert = DeviceCert::sign(
            &root,
            account,
            device,
            &device_sk.public_key(),
            &KemPublicKey::from([0x33; 32]),
            0,
            0,
        )
        .expect("the certificate must sign");

        let proof = AccountProof {
            genesis,
            chain: vec![],
            statement: cert,
        };

        (
            account,
            device_sk.public_key(),
            hex::encode(borsh::to_vec(&proof).expect("the proof must encode")),
        )
    }

    /// The point of the whole change: a delegated caller presenting its own
    /// credential is told which device it is, and it is the signing key the
    /// nonce ledger is keyed by.
    #[test]
    fn a_caller_presenting_its_own_proof_is_resolved_to_its_device_key() {
        let (account, device_key, proof) = account_with_device(1, 5);

        assert_eq!(
            authorised_device(account, &proof).expect("its own proof must be accepted"),
            device_key
        );
    }

    /// Whitespace round a hex blob is the ordinary cost of a credential that
    /// travels through copy-paste and shell pipelines; `perform_intent` trims
    /// too, and a recovery route that refused what the write path accepts would
    /// be a trap.
    #[test]
    fn surrounding_whitespace_does_not_refuse_a_good_proof() {
        let (account, device_key, proof) = account_with_device(1, 5);

        assert_eq!(
            authorised_device(account, &format!("  {proof}\n")).expect("must be accepted"),
            device_key
        );
    }

    /// A well-formed proof for somebody ELSE's account must not resolve. This is
    /// the oracle the admin-only placement was protecting against, and it is
    /// closed here rather than by hiding the route.
    #[test]
    fn a_proof_for_another_account_is_refused() {
        let (_other_account, _other_device, other_proof) = account_with_device(2, 6);
        let (mine, _my_device, _my_proof) = account_with_device(1, 5);

        assert!(
            authorised_device(mine, &other_proof).is_err(),
            "presenting another account's credential must not read that account's device"
        );
    }

    /// And it must be refused *identically* to garbage. Told apart, the route
    /// answers "is this credential real?" for a proof the caller captured but
    /// does not own — an oracle rebuilt in the error channel.
    #[test]
    fn a_wrong_account_and_a_malformed_proof_are_the_same_refusal() {
        let (_other, _key, other_proof) = account_with_device(2, 6);
        let (mine, _my_key, _my_proof) = account_with_device(1, 5);

        let wrong_account = authorised_device(mine, &other_proof).expect_err("must be refused");

        for garbage in ["", "zz", "not hex at all", &hex::encode([0xAB; 64])] {
            let malformed = authorised_device(mine, garbage).expect_err("must be refused");
            assert_eq!(
                malformed.status_code, wrong_account.status_code,
                "`{garbage}` must not be distinguishable from a valid proof for another account"
            );
            assert_eq!(
                malformed.message, wrong_account.message,
                "`{garbage}` must not be distinguishable from a valid proof for another account"
            );
        }
    }

    /// A refused credential is the caller's problem, never a `500`: a node
    /// reported as broken invites a retry of something that can never succeed.
    #[test]
    fn a_refused_credential_is_a_client_error() {
        let (mine, _key, _proof) = account_with_device(1, 5);

        let refusal = authorised_device(mine, "not a credential").expect_err("must be refused");
        assert!(refusal.status_code.is_client_error());
    }

    /// A certificate whose signature has been tampered with must not resolve,
    /// even though every field still parses and names the right account. The
    /// root signature is the entire authority here.
    #[test]
    fn a_proof_with_a_broken_signature_is_refused() {
        let root = PrivateKey::from([1u8; 32]);
        let device_sk = PrivateKey::from([5u8; 32]);
        let genesis = AccountGenesis::new(root.public_key());
        let account = genesis.account_id();

        let mut cert = DeviceCert::sign(
            &root,
            account,
            DeviceId::mint(account, [5u8; 16]),
            &device_sk.public_key(),
            &KemPublicKey::from([0x33; 32]),
            0,
            0,
        )
        .expect("the certificate must sign");
        cert.signature[0] ^= 0xFF;

        let proof = AccountProof {
            genesis,
            chain: vec![],
            statement: cert,
        };
        let encoded = hex::encode(borsh::to_vec(&proof).expect("the proof must encode"));

        assert!(authorised_device(account, &encoded).is_err());
    }

    /// The device key is taken from the verified certificate and is the key the
    /// ledger is keyed by, so the two routes answer about the same thing: the
    /// resolved key, fed to `describe`, produces exactly what the admin `GET`
    /// produces for that key.
    #[test]
    fn both_routes_describe_the_same_device() {
        let (account, device_key, proof) = account_with_device(1, 5);
        let resolved = authorised_device(account, &proof).expect("must be accepted");

        let admin = describe(context(), device_key, Some(ContextWarrantNonce::first(7)));
        let delegated = describe(context(), resolved, Some(ContextWarrantNonce::first(7)));

        assert_eq!(admin.author_device_key, delegated.author_device_key);
        assert_eq!(admin.next_nonce, delegated.next_nonce);
    }

    /// The two cases a recovering client actually lands in, reached through the
    /// delegated route's own resolution rather than a hand-made key: a device
    /// that has never written here starts at the bottom, and one that has spent
    /// everything is told there is no next nonce rather than handed a wrapped
    /// zero.
    #[test]
    fn a_delegated_read_reports_the_fresh_and_exhausted_cases_unchanged() {
        let (account, _key, proof) = account_with_device(1, 5);
        let resolved = authorised_device(account, &proof).expect("must be accepted");

        let fresh = describe(context(), resolved, None);
        assert!(!fresh.seen);
        assert_eq!(fresh.next_nonce, Some(0));

        let exhausted = describe(
            context(),
            resolved,
            Some(ContextWarrantNonce::first(u64::MAX)),
        );
        assert_eq!(exhausted.next_nonce, None);
    }

    /// The request body has exactly one field and refuses anything else, so a
    /// client that tries to name a device key is told rather than quietly served
    /// the certificate's key instead.
    #[test]
    fn the_request_names_no_device_key() {
        use calimero_server_primitives::admin::WarrantNonceApiRequest;

        let good: Result<WarrantNonceApiRequest, _> =
            serde_json::from_str(r#"{"authorProof":"ab"}"#);
        assert!(
            good.is_ok(),
            "the one field a caller sends must deserialise"
        );

        let with_device: Result<WarrantNonceApiRequest, _> = serde_json::from_str(
            r#"{"authorProof":"ab","authorDeviceKey":"11111111111111111111111111111111"}"#,
        );
        assert!(
            with_device.is_err(),
            "a device key in the body must be refused, not ignored"
        );
    }
}
