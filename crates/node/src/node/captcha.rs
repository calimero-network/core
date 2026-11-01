//! Human checks before a component's call: a method the ABI marks `captcha(turnstile)`
//! runs for a client only after the node it reached verified the client's Cloudflare
//! Turnstile token (`Node::verify_captcha`, from `/intents`).
//!
//! The check is the app's own outbound request, made by the node: it goes through
//! `Node::outbound_for` with the app's identity, so every gate of an action's `fetch`
//! holds (S9, R14). The app's signed manifest declares
//! [`calimero_outbound::TURNSTILE_VERIFY_URL`] under `outbound`, its owner approved it,
//! the operator allows `challenges.cloudflare.com` and holds the site's secret as
//! [`TURNSTILE_SECRET`] for the app's package and signer. The secret is filled in by the
//! outbound service and never seen by the app or this code.
//!
//! Turnstile spends a token on its first check. The call's idempotency key goes with the
//! check (as a UUID derived from the app, the caller and the key), so a client's retry
//! with the same key passes again, and the node then replays the call it logged.

use calimero_app_host::Captcha;
use calimero_primitives::outbound::{OutboundMethod, OutboundRequest, OutboundResponse};
use ring::digest::{digest, SHA256};
use serde_json::{json, Value};

use super::Node;
use crate::error::ApiError;
use crate::types::Caller;

/// The secret name the operator gives the site's Turnstile secret key.
pub const TURNSTILE_SECRET: &str = "TURNSTILE_SECRET";
/// Bytes of a client's token; Turnstile's are at most 2048 characters.
pub const MAX_CAPTCHA_TOKEN_BYTES: usize = 2048;
/// Error codes kept from a failed check's answer.
const MAX_ERROR_CODES: usize = 8;

impl Node {
    /// Verify `token` for a client's call of a `captcha` method of `app_id` (running
    /// `bundle`) with `idempotency_key`.
    ///
    /// # Errors
    ///
    /// `400 CAPTCHA_REQUIRED` without a token; `403 CAPTCHA_FAILED` when the provider
    /// refuses it (with its `errorCodes`) or it is too long; `503 CAPTCHA_UNAVAILABLE` when
    /// the check cannot be made (no signed release declaring the URL, not approved, no
    /// secret, the provider unreachable).
    pub(crate) async fn verify_captcha(
        &self,
        app_id: &str,
        bundle: &str,
        captcha: Captcha,
        caller: &Caller,
        token: Option<&str>,
        idempotency_key: Option<&str>,
    ) -> Result<(), ApiError> {
        let Captcha::Turnstile = captcha;
        let token = token.filter(|t| !t.is_empty()).ok_or_else(|| {
            ApiError::new(
                400,
                "CAPTCHA_REQUIRED",
                "this method needs a solved Turnstile challenge: send its token as `captcha`",
            )
            .with_details(json!({ "provider": "turnstile" }))
        })?;
        if token.len() > MAX_CAPTCHA_TOKEN_BYTES {
            return Err(failed(&[]));
        }
        let meter = self.meter(app_id, bundle);
        let Some(outbound) = self.outbound_for(
            app_id,
            bundle,
            self.usage.outbound(&meter.app, &meter.payer),
        ) else {
            return Err(unavailable(&format!(
                "the app's signed release must declare {} under `outbound`, and this \
                 node's operator must turn outbound requests on",
                calimero_outbound::TURNSTILE_VERIFY_URL
            )));
        };
        // Built apart: the serializer is not `Send` and must not live across an await.
        let fields = {
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            let _ = form.append_pair("response", token);
            if let Some(key) = idempotency_key {
                let _ = form.append_pair("idempotency_key", &check_key(app_id, caller, key));
            }
            form.finish()
        };
        let request = OutboundRequest {
            method: OutboundMethod::Post,
            url: calimero_outbound::TURNSTILE_VERIFY_URL.to_owned(),
            headers: vec![(
                "Content-Type".to_owned(),
                "application/x-www-form-urlencoded".to_owned(),
            )],
            // The secret's placeholder stays unencoded: the outbound service fills it in.
            body: format!("secret={{{{secret:{TURNSTILE_SECRET}}}}}&{fields}").into_bytes(),
        };
        let request = borsh::to_vec(&request).map_err(|e| unavailable(&e.to_string()))?;
        let sent = tokio::task::spawn_blocking(move || outbound.send(&request))
            .await
            .map_err(|e| unavailable(&e.to_string()))?
            .map_err(|reason| unavailable(&reason))?;
        let response: OutboundResponse =
            borsh::from_slice(&sent).map_err(|e| unavailable(&e.to_string()))?;
        if response.status != 200 {
            return Err(unavailable(&format!(
                "Turnstile answered {}",
                response.status
            )));
        }
        let answer: Value = serde_json::from_slice(&response.body)
            .map_err(|e| unavailable(&format!("Turnstile's answer: {e}")))?;
        if answer["success"] == true {
            return Ok(());
        }
        let codes: Vec<String> = answer["error-codes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(MAX_ERROR_CODES)
            .map(|c| c.chars().take(64).collect())
            .collect();
        // A wrong or missing secret is the operator's to fix, not the client's.
        if codes
            .iter()
            .any(|c| c == "missing-input-secret" || c == "invalid-input-secret")
        {
            return Err(unavailable(
                "this node's Turnstile secret for the app is wrong",
            ));
        }
        Err(failed(&codes))
    }
}

fn failed(codes: &[String]) -> ApiError {
    ApiError::new(
        403,
        "CAPTCHA_FAILED",
        "the Turnstile challenge was not passed",
    )
    .with_details(json!({ "provider": "turnstile", "errorCodes": codes }))
}

fn unavailable(reason: &str) -> ApiError {
    ApiError::new(
        503,
        "CAPTCHA_UNAVAILABLE",
        "the node could not verify the Turnstile challenge",
    )
    .with_details(json!({ "provider": "turnstile", "reason": reason }))
}

/// Turnstile's `idempotency_key` (a UUID) for the call keyed `key` by `caller` to
/// `app_id`: the same call retried checks the same, another call never shares it.
fn check_key(app_id: &str, caller: &Caller, key: &str) -> String {
    let caller = serde_json::to_string(caller).unwrap_or_default();
    let mut input = Vec::new();
    for part in ["calimero.captcha.v1", app_id, caller.as_str(), key] {
        input.extend_from_slice(&(part.len() as u64).to_be_bytes());
        input.extend_from_slice(part.as_bytes());
    }
    let mut b = [0_u8; 16];
    b.copy_from_slice(&digest(&SHA256, &input).as_ref()[..16]);
    // A version 4, RFC 4122 variant UUID.
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_keys_are_uuids_one_per_call() {
        let alice = Caller::Public;
        let k = check_key("app", &alice, "k1");
        assert_eq!(k.len(), 36);
        assert_eq!(&k[14..15], "4");
        assert_eq!(k, check_key("app", &alice, "k1"));
        assert_ne!(k, check_key("app", &alice, "k2"));
        assert_ne!(k, check_key("other", &alice, "k1"));
    }
}
