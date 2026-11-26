//! First-party connectors: a call to a service that matters (Stripe, an OpenAI-compatible
//! API) that the node, not the app, builds.
//!
//! The app names a connector its release's signed manifest declares (`connectors`), one of
//! the operations declared for it and a JSON input; the node checks the input against the
//! operation's typed fields, builds the HTTP request, attaches the credential and answers
//! only the operation's declared fields. Anything else (an undeclared connector or
//! operation, a field the operation does not take, a value out of bounds or not allowed,
//! egress off, no key bound) is refused before anything leaves the node. See
//! [Use a first-party connector](https://calimero-network.github.io/core/build/guides/connectors/).
//!
//! ```ignore
//! let intent = connector::call(
//!     "stripe",
//!     "createPaymentIntent",
//!     &json!({ "amount": 2000, "currency": "usd", "metadata": { "order_id": order.id } }),
//! )?;
//! ```
//!
//! Actions, calls, timers and xcall deliveries may call (a logged run's entry records the
//! call and its answer); views and `init` may not. Each call counts as one of the run's
//! outbound requests (8). On the host target (`env::mock`) every call is refused; the
//! testing harness answers them with `TestApp::mock_connector`.

use serde::Serialize;
use serde_json::{json, Value};

use crate::error::{AppError, Result};

/// Calls `operation` of the node's connector `connector` with `input` (serialized as
/// JSON); the operation's declared answer fields.
///
/// # Errors
/// `Connector { message }`: the node refused the call (the message says why), the
/// service answered an error (its status and error code only), or the answer was not
/// JSON.
pub fn call<I: Serialize + ?Sized>(connector: &str, operation: &str, input: &I) -> Result<Value> {
    let input = serde_json::to_vec(input).map_err(|e| connector_error(e.to_string()))?;
    let answer = call_raw(connector, operation, &input)?;
    serde_json::from_slice(&answer).map_err(|e| connector_error(e.to_string()))
}

/// [`call`] with the input already JSON, answering the node's JSON as it came.
///
/// # Errors
/// `Connector { message }`, as [`call`].
pub fn call_raw(connector: &str, operation: &str, input: &[u8]) -> Result<Vec<u8>> {
    crate::env::backend_connector_call(connector, operation, input).map_err(connector_error)
}

fn connector_error(message: String) -> AppError {
    AppError::with_fields("Connector", json!({ "message": message }))
}
