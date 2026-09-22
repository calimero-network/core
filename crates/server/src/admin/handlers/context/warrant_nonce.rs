//! `GET /admin-api/contexts/:context_id/warrant-nonce/:author_device_key` —
//! where an author device stands in its warrant-nonce sequence here.
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

use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{WarrantNonceApiResponse, WarrantNonceApiResponseData};
use calimero_store::types::ContextWarrantNonce;
use reqwest::StatusCode;
use tracing::error;

use crate::admin::service::{ApiError, ApiResponse};
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

#[cfg(test)]
mod tests {
    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::types::ContextWarrantNonce;

    use super::{describe, parse_device_key};

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
}
