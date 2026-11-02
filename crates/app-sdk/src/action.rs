//! Actions (spec D12, D17, D22, D27): methods that run outside the log, on the leader, with
//! outbound HTTP and no state access.
//!
//! ```ignore
//! #[app::action]
//! pub fn enrich_company(ctx: &mut ActionCtx, company_id: String) -> app::Result<()> {
//!     let company: Company = ctx.view("company", &json!({ "id": company_id }))?;
//!     let res = ctx.fetch(&Request::get(format!("https://enrich.example.com/v1/{}", company.domain))
//!         .header("Idempotency-Key", &ctx.key()))?;
//!     let info: Enrichment = res.json()?;
//!     ctx.call::<()>("applyEnrichment", &json!({ "id": company_id, "industry": info.industry }))?;
//!     Ok(())
//! }
//! ```
//!
//! A client starts one through the API (`POST /apps/{app}/intents` naming it), and a call
//! or a timer with [`crate::env::start_action`]. The platform logs that it started, runs it,
//! retries a timeout (5 attempts) and logs that it ended; an app error or a trap fails it at
//! once (D22). Each `call` is its own log entry under a key derived from the action's key,
//! the method, the arguments and how many identical calls came before (D27), so a retried
//! action's calls replay instead of applying twice. `fetch` reaches only hosts the app's
//! signed manifest declares and its owner approved (S9); `{{secret:NAME}}` in a header or
//! the body is filled in by the node (R14). Only the leader runs actions: they are
//! at-least-once, so send `ctx.key()` as the remote side's idempotency key.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;

use crate::env;
use crate::error::{AppError, Result};

/// The context of an action run, its first parameter.
#[derive(Debug)]
pub struct ActionCtx {
    _private: (),
}

/// An outgoing HTTP request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// `GET`, `POST`, ..
    pub method: String,
    /// `https://host/path?query`.
    pub url: String,
    /// Header names and values.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl Request {
    /// A `GET` of `url`.
    #[must_use]
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: "GET".into(),
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// A `POST` of `body` to `url`.
    #[must_use]
    pub fn post(url: impl Into<String>, body: impl Into<Vec<u8>>) -> Self {
        Self {
            method: "POST".into(),
            body: body.into(),
            ..Self::get(url)
        }
    }

    /// Adds a header.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

/// An HTTP response, read whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// The status code.
    pub status: u16,
    /// Header names and values.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl Response {
    /// The body as JSON.
    ///
    /// # Errors
    ///
    /// `Fetch` when it is not.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(|e| fetch_error(e.to_string()))
    }

    /// The body as text (invalid UTF-8 replaced).
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

fn fetch_error(message: String) -> AppError {
    AppError::with_fields("Fetch", json!({ "message": message }))
}

impl ActionCtx {
    /// The context of the running action. Called by the code `#[app::action]` generates.
    #[doc(hidden)]
    #[must_use]
    pub fn current() -> Self {
        Self { _private: () }
    }

    /// This action's key: the same on every attempt (D17). Send it as the idempotency key
    /// of outbound requests.
    #[must_use]
    pub fn key(&self) -> String {
        env::backend_action_key()
    }

    /// Run view `method` of this app with `args`; its JSON result.
    ///
    /// # Errors
    ///
    /// `ActionView` with the platform's reason, or the result not decoding as `T`.
    pub fn view<T: DeserializeOwned, A: Serialize + ?Sized>(
        &mut self,
        method: &str,
        args: &A,
    ) -> Result<T> {
        let args = encode(args)?;
        let out = env::backend_action_view(method, &args)
            .map_err(|m| AppError::with_fields("ActionView", json!({ "message": m })))?;
        decode(&out)
    }

    /// Run `method` of this app as one logged call with the action's caller (D13), keyed
    /// for at-least-once (D27); its JSON result.
    ///
    /// # Errors
    ///
    /// `ActionCall` with the platform's answer (the method's own error included), or the
    /// result not decoding as `T`.
    pub fn call<T: DeserializeOwned, A: Serialize + ?Sized>(
        &mut self,
        method: &str,
        args: &A,
    ) -> Result<T> {
        let args = encode(args)?;
        let out = env::backend_action_call(method, &args)
            .map_err(|m| AppError::with_fields("ActionCall", json!({ "message": m })))?;
        decode(&out)
    }

    /// Send `request` over `wasi:http` (S9): only to a host the app's signed manifest
    /// declares and its owner approved; the response is read whole.
    ///
    /// # Errors
    ///
    /// `Fetch` with the reason it was not sent or failed.
    pub fn fetch(&mut self, request: &Request) -> Result<Response> {
        env::backend_fetch(request).map_err(fetch_error)
    }
}

fn encode<A: Serialize + ?Sized>(args: &A) -> Result<Vec<u8>> {
    serde_json::to_vec(args)
        .map_err(|e| AppError::with_fields("BadArgs", json!({ "message": e.to_string() })))
}

fn decode<T: DeserializeOwned>(out: &[u8]) -> Result<T> {
    let out = if out.is_empty() {
        b"null".as_slice()
    } else {
        out
    };
    serde_json::from_slice(out)
        .map_err(|e| AppError::with_fields("BadOutput", json!({ "message": e.to_string() })))
}
