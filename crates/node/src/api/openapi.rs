//! The node API as an OpenAPI 3.1 document: `docs/public/openapi.json`, the docs site's API
//! reference. [`operations`] is the registry, one entry per public, auth and ops route of
//! [`super::router`]; [`document`] builds the document from it.
//!
//! The tests hold the registry to the code, so the document cannot drift from it:
//!
//! - every route of `router` outside `/internal/` has exactly one entry, and no entry names a
//!   route `router` lacks (read from this crate's source);
//! - an entry's request body and query are the ones its handler extracts (`Json<T>`,
//!   `Option<Json<T>>`, `Query<T>`, raw bytes), and their shapes match `T`: the same field
//!   names (from `T`'s `Deserialize`), a sample of the shape deserializes into `T`, and a
//!   field is optional in the shape exactly when `T` accepts the body without it, at any
//!   depth: each struct `T` reads inside the body, in a list or a property, is held to the
//!   object the shape has there ([`trace`]);
//! - each error code the handler raises with a literal code is listed for it;
//! - the committed document is the one [`document`] builds now. `UPDATE_OPENAPI=1 cargo
//!   test -p calimero-node --lib openapi` rewrites it.
//!
//! Response shapes are written here by hand from the handlers' answers. A component the node
//! answers by serializing a Rust type names that type (`Component::ty`), and is held to it:
//! the same field names (of each struct it reads, at any depth), a sample of the shape read
//! into the type writes back as a value of the shape, and a field is optional in the shape
//! exactly when the type may leave it out.
//! The rest, built with `json!`, are not checked.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};
use serde::Serialize;
use serde_json::{json, Map, Value};

mod shape;
mod trace;

use shape::{parse, Shape};

/// Who may call a route: the terms of the "Who" column in `operate/admin-api.mdx`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    /// No credential.
    Anyone,
    /// A session or an API key.
    SignedIn,
    /// A passkey session; an API key is refused.
    Session,
    /// A passkey session not bound to an app's frontend.
    AccountHolder,
    /// An account holder, or an API key scoped to no app or method and not read-only.
    Installer,
    /// Whoever the app's access setting admits; `public` admits anyone.
    AppAccess,
    /// The ops token.
    Ops,
}

impl Who {
    fn text(self) -> &'static str {
        match self {
            Self::Anyone => "anyone",
            Self::SignedIn => "signed in (a session or an API key)",
            Self::Session => "a passkey session (an API key is refused)",
            Self::AccountHolder => {
                "an account holder (a passkey session not bound to an app's frontend)"
            }
            Self::Installer => {
                "an installer (an account holder, or an unscoped API key that is not read-only)"
            }
            Self::AppAccess => "whoever the app's access admits (a public app: anyone)",
            Self::Ops => "the ops token",
        }
    }

    fn security(self) -> Value {
        match self {
            Self::Anyone => json!([]),
            Self::SignedIn | Self::Installer => json!([{ "session": [] }, { "apiKey": [] }]),
            Self::Session | Self::AccountHolder => json!([{ "session": [] }]),
            Self::AppAccess => json!([{ "session": [] }, { "apiKey": [] }, {}]),
            Self::Ops => json!([{ "opsToken": [] }]),
        }
    }
}

/// A request body or query and the type its handler reads it into.
struct Input {
    shape: &'static str,
    /// `T` of the handler's extractor, for the checks; `None` for `Value`.
    ty: Option<Ty>,
}

/// What the checks need of a Rust type.
#[derive(Clone, Copy)]
struct Ty {
    name: &'static str,
    /// Deserializes a JSON body (or, for a query, a query string) into the type.
    parse: fn(&Value, bool) -> Result<(), String>,
    /// The field names its `Deserialize` reads, when it is a plain struct.
    fields: fn() -> Option<&'static [&'static str]>,
    /// The structs it reads from a JSON body, at any depth.
    structs: fn(&Value) -> Result<Vec<trace::Read>, String>,
}

fn ty<T: DeserializeOwned>() -> Ty {
    fn parse<T: DeserializeOwned>(v: &Value, query: bool) -> Result<(), String> {
        if query {
            let qs = query_string(v);
            let uri: axum::http::Uri = format!("/?{qs}").parse().map_err(|e| format!("{e}"))?;
            return axum::extract::Query::<T>::try_from_uri(&uri)
                .map(drop)
                .map_err(|e| e.body_text());
        }
        serde_json::from_value::<T>(v.clone())
            .map(drop)
            .map_err(|e| e.to_string())
    }
    Ty {
        name: std::any::type_name::<T>(),
        parse: parse::<T>,
        fields: fields_of::<T>,
        structs: trace::structs::<T>,
    }
}

/// `v`'s fields as a query string (`k=v&..`, strings unquoted).
fn query_string(v: &Value) -> String {
    v.as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| match v {
                    Value::String(s) => format!("{k}={s}"),
                    other => format!("{k}={other}"),
                })
                .collect::<Vec<_>>()
                .join("&")
        })
        .unwrap_or_default()
}

/// The request's content.
enum Body {
    None,
    /// `Json<T>`.
    Json(Input),
    /// `Option<Json<T>>`: no body at all is the same as an empty one.
    OptionalJson(Input),
    /// The raw bytes (`axum::body::Bytes`), of this media type.
    Raw(&'static str, &'static str),
    /// Raw bytes the handler reads as optional JSON of `T` (an empty body is none).
    RawJson(Input),
    /// Raw bytes the handler deserializes as JSON of `T` itself (after capping their size).
    BytesJson(Input),
}

/// The success answer.
enum Reply {
    Json(&'static str),
    /// Server-sent events: what they carry.
    Events(&'static str),
    /// `text/plain`.
    Text(&'static str),
    /// Raw bytes of this media type (`*/*`: whichever the handler names).
    Bytes(&'static str, &'static str),
}

/// Whether a client may send the same request again after a `503` (or a lost answer).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Retry {
    /// Not said: it may have taken effect.
    Unsaid,
    /// Always: it changes nothing, or the same request is answered the same.
    Safe,
    /// When the body carries this field (an idempotency key).
    SafeWith(&'static str),
}

/// One operation of the API.
struct Op {
    method: &'static str,
    path: &'static str,
    /// The handler as `router` names it, for the checks and the `operationId`.
    handler: &'static str,
    who: Who,
    summary: &'static str,
    description: &'static str,
    body: Body,
    query: Option<Input>,
    reply: Reply,
    /// `412 CONFIRM_REQUIRED` first: a passkey assertion over the exact request.
    step_up: bool,
    /// When the step-up applies, if not always (`x-calimero-step-up-when`).
    step_up_when: &'static str,
    /// The WebAuthn `userVerification` the step-up's assertion is checked with.
    user_verification: &'static str,
    /// `428 OPERATOR_SIGNATURE_REQUIRED` first: the operator key's signature.
    operator: bool,
    retry: Retry,
    errors: &'static [(u16, &'static str)],
}

fn op(
    method: &'static str,
    path: &'static str,
    handler: &'static str,
    who: Who,
    summary: &'static str,
) -> Op {
    Op {
        method,
        path,
        handler,
        who,
        summary,
        description: "",
        body: Body::None,
        query: None,
        reply: Reply::Json("{}"),
        step_up: false,
        step_up_when: "",
        user_verification: "required",
        operator: false,
        retry: Retry::Unsaid,
        errors: &[],
    }
}

fn get(path: &'static str, handler: &'static str, who: Who, summary: &'static str) -> Op {
    op("get", path, handler, who, summary)
}

fn post(path: &'static str, handler: &'static str, who: Who, summary: &'static str) -> Op {
    op("post", path, handler, who, summary)
}

fn put(path: &'static str, handler: &'static str, who: Who, summary: &'static str) -> Op {
    op("put", path, handler, who, summary)
}

fn delete(path: &'static str, handler: &'static str, who: Who, summary: &'static str) -> Op {
    op("delete", path, handler, who, summary)
}

impl Op {
    fn about(mut self, description: &'static str) -> Self {
        self.description = description;
        self
    }

    fn json<T: DeserializeOwned>(mut self, shape: &'static str) -> Self {
        self.body = Body::Json(Input {
            shape,
            ty: Some(ty::<T>()),
        });
        self
    }

    fn opt_json<T: DeserializeOwned>(mut self, shape: &'static str) -> Self {
        self.body = Body::OptionalJson(Input {
            shape,
            ty: Some(ty::<T>()),
        });
        self
    }

    /// An `Option<Json<Value>>` body: any JSON, read field by field.
    fn opt_value(mut self, shape: &'static str) -> Self {
        self.body = Body::OptionalJson(Input { shape, ty: None });
        self
    }

    /// A `Json<Value>` body: any JSON, read field by field.
    fn value(mut self, shape: &'static str) -> Self {
        self.body = Body::Json(Input { shape, ty: None });
        self
    }

    fn raw(mut self, media: &'static str, what: &'static str) -> Self {
        self.body = Body::Raw(media, what);
        self
    }

    /// An operator decision's raw body: nothing, or `{ operatorProof }`.
    fn proof_body(mut self) -> Self {
        self.body = Body::RawJson(Input {
            shape: "#ProofOnly",
            ty: Some(ty::<super::OpsProofBody>()),
        });
        self
    }

    /// Raw bytes the handler deserializes as JSON of `T` (required).
    fn bytes_json<T: DeserializeOwned>(mut self, shape: &'static str) -> Self {
        self.body = Body::BytesJson(Input {
            shape,
            ty: Some(ty::<T>()),
        });
        self
    }

    fn query<T: DeserializeOwned>(mut self, shape: &'static str) -> Self {
        self.query = Some(Input {
            shape,
            ty: Some(ty::<T>()),
        });
        self
    }

    fn ok(mut self, shape: &'static str) -> Self {
        self.reply = Reply::Json(shape);
        self
    }

    fn events(mut self, what: &'static str) -> Self {
        self.reply = Reply::Events(what);
        self
    }

    fn text(mut self, what: &'static str) -> Self {
        self.reply = Reply::Text(what);
        self
    }

    /// Raw bytes of whichever media type the handler names in `Content-Type`.
    fn any_bytes(mut self, what: &'static str) -> Self {
        self.reply = Reply::Bytes("*/*", what);
        self
    }

    fn step_up(mut self) -> Self {
        self.step_up = true;
        self
    }

    /// A step-up only `when` this holds; otherwise the request is answered at once.
    fn step_up_when(mut self, when: &'static str) -> Self {
        self.step_up = true;
        self.step_up_when = when;
        self
    }

    /// A step-up whose assertion needs the user present, not verified (`rp`, not
    /// `rp_step_up`): a sign-up's confirmation.
    fn step_up_uv_preferred(mut self) -> Self {
        self.step_up = true;
        self.user_verification = "preferred";
        self
    }

    fn retry_safe(mut self) -> Self {
        self.retry = Retry::Safe;
        self
    }

    fn retry_safe_with(mut self, field: &'static str) -> Self {
        self.retry = Retry::SafeWith(field);
        self
    }

    fn operator(mut self) -> Self {
        self.operator = true;
        self
    }

    fn errors(mut self, errors: &'static [(u16, &'static str)]) -> Self {
        self.errors = errors;
        self
    }

    /// Every error code the operation is documented to answer, by status.
    fn codes(&self) -> BTreeMap<u16, BTreeSet<&'static str>> {
        let mut out: BTreeMap<u16, BTreeSet<&'static str>> = BTreeMap::new();
        let mut add = |status: u16, code: &'static str| {
            let _new = out.entry(status).or_default().insert(code);
        };
        for &(status, code) in self.errors {
            add(status, code);
        }
        match self.who {
            Who::Anyone => {}
            Who::AppAccess | Who::SignedIn => add(401, "UNAUTHENTICATED"),
            Who::Ops | Who::Session | Who::AccountHolder | Who::Installer => {
                add(401, "UNAUTHENTICATED");
                add(403, "FORBIDDEN");
            }
        }
        // A session this node's release may not serve (`verify_token`).
        if !matches!(self.who, Who::Anyone | Who::Ops) {
            add(421, "WRONG_NODE");
        }
        if self.step_up {
            add(412, "CONFIRM_REQUIRED");
            add(400, "UNKNOWN_CHALLENGE");
            add(400, "EXPIRED_CHALLENGE");
        }
        if self.operator {
            add(428, "OPERATOR_SIGNATURE_REQUIRED");
            add(403, "BAD_OPERATOR_SIGNATURE");
        }
        out
    }
}

/// A shared shape: `#Name` in a shape.
struct Component {
    name: &'static str,
    description: &'static str,
    shape: &'static str,
    /// The Rust type the node serializes into this shape, for the checks.
    ty: Option<OutTy>,
    /// A request body whose type refuses a property the shape does not name.
    closed_body: bool,
}

const fn component(
    name: &'static str,
    description: &'static str,
    shape: &'static str,
) -> Component {
    Component {
        name,
        description,
        shape,
        ty: None,
        closed_body: false,
    }
}

impl Component {
    /// A request body whose type refuses unknown properties (`additionalProperties: false`).
    fn closed_body(mut self) -> Self {
        self.closed_body = true;
        self
    }

    /// The node answers this shape by serializing a `T`.
    fn of<T: DeserializeOwned + Serialize>(mut self) -> Self {
        self.ty = Some(out_ty::<T>());
        self
    }
}

/// What the response checks need of a Rust type the node answers with.
#[derive(Clone, Copy)]
struct OutTy {
    name: &'static str,
    /// Reads a JSON value into the type and writes it back: what the node would answer.
    round_trip: fn(&Value) -> Result<Value, String>,
    /// The field names its `Deserialize` reads, when it is a plain struct.
    fields: fn() -> Option<&'static [&'static str]>,
    /// The structs it reads from a JSON value, at any depth.
    structs: fn(&Value) -> Result<Vec<trace::Read>, String>,
}

fn out_ty<T: DeserializeOwned + Serialize>() -> OutTy {
    fn round_trip<T: DeserializeOwned + Serialize>(v: &Value) -> Result<Value, String> {
        let t: T = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        serde_json::to_value(&t).map_err(|e| e.to_string())
    }
    OutTy {
        name: std::any::type_name::<T>(),
        round_trip: round_trip::<T>,
        fields: fields_of::<T>,
        structs: trace::structs::<T>,
    }
}

/// The shape of `details` by error code: each code's `details`, where it carries one, is a
/// value of this component. `error_details_match_their_constructors` holds each to the
/// constructor that builds it.
const ERROR_DETAILS: &[(&str, &str)] = &[
    ("APP_ERROR", "AppError"),
    ("BAD_BUNDLE", "ServiceRefused"),
    ("COMPUTE_ABI_MISMATCH", "ServiceRefused"),
    ("CONFIRM_REQUIRED", "ConfirmRequired"),
    ("FEATURE_NOT_ACTIVE", "FeatureNotActive"),
    ("FILES_UNAVAILABLE", "RetryAfter"),
    ("INIT_FAILED", "InitFailed"),
    ("NOT_LEADER", "NotLeader"),
    ("OPERATOR_SIGNATURE_REQUIRED", "OperatorSignatureRequired"),
    ("PART_CONFLICT", "UploadPosition"),
    ("PART_OUT_OF_ORDER", "UploadPosition"),
    ("RECOVERY_NOT_READY", "RecoveryNotReady"),
    ("RELEASE_DEADLINE", "ReleaseDeadline"),
    ("RUNTIME_TOO_OLD", "RuntimeTooOld"),
    ("TIMEOUT", "RetryAfter"),
    ("TRAP", "Trap"),
    ("UNSUPPORTED_BUNDLE", "ServiceRefused"),
    ("UPGRADE_FAILED", "Applied"),
    ("UPGRADE_PENDING", "UpgradePending"),
    ("UPLOAD_INCOMPLETE", "UploadPosition"),
];

/// `x-calimero-details` of these codes: each code's `details` schema, by reference.
fn details_of<'a>(codes: impl IntoIterator<Item = &'a str>) -> Map<String, Value> {
    codes
        .into_iter()
        .filter_map(|code| {
            let (_, name) = ERROR_DETAILS.iter().find(|(c, _)| *c == code)?;
            Some((
                code.to_owned(),
                json!({ "$ref": format!("#/components/schemas/{name}") }),
            ))
        })
        .collect()
}

/// Codes answered on many routes, and what they mean (the Errors table of `admin-api.mdx`).
const COMMON_CODES: &[(u16, &str, &str)] = &[
    (400, "BAD_REQUEST", "the request is malformed"),
    (
        400,
        "REJECTED",
        "the system log refused the command when it applied; `message` says why",
    ),
    (
        401,
        "UNAUTHENTICATED",
        "no, unknown, expired or revoked credential",
    ),
    (
        403,
        "FORBIDDEN",
        "the credential may not do this (scope, role, governance, read-only)",
    ),
    (
        404,
        "NOT_FOUND",
        "no such object, or one the caller may not see",
    ),
    (409, "CONFLICT", "the state does not allow it now"),
    (
        412,
        "CONFIRM_REQUIRED",
        "a passkey assertion is needed; `details` carries the challenge",
    ),
    (
        421,
        "NOT_LEADER",
        "send it to the leader named in `details.leaderUrl`",
    ),
    (
        421,
        "WRONG_NODE",
        "this node runs a release the account's owners have not approved, so it neither issues nor takes the account's sessions; send it to another node of the cluster (no `details`; nothing was spent)",
    ),
    (
        428,
        "OPERATOR_SIGNATURE_REQUIRED",
        "the operator key must sign the decision; `details` names the statement",
    ),
    (
        429,
        "RATE_LIMITED",
        "a node's limit on open streams or ceremonies",
    ),
    (503, "UNAVAILABLE", "the cluster cannot answer now"),
];

const DESCRIPTION: &str = "\
Every route a `merod` node serves, on one HTTP API at its `--listen` address. The route \
table is `router` in `crates/node/src/api.rs`; this document is generated from the registry \
next to it (`crates/node/src/api/openapi.rs`), whose tests hold it to that table.\n\n\
**Requests are sealed.** Only `GET /health` and `GET /attestation` travel in the clear. \
Every other request goes inside the sealed channel (`calimero-sealed`, wire format v2, \
Noise NK) to the transport key the node's attestation binds, after the client checked the \
quote against a mero-tee release it trusts; a TDX node refuses plain requests (a node \
started with `--dev` accepts them). A generic HTTP tool (curl, Postman, a generated \
client) therefore reaches only those two routes: use a sealing client (`meroctl`, \
`calimero-client` for Rust, mero-js `CalimeroClient`, calimero-client-py `CloudClient`), \
or put such a client's transport under the generated code. The paths, bodies and answers \
below are what travels inside the channel.\n\n\
**Errors** answer `{ status, error, message, details? }`: `error` is the code, `details` \
says more (the challenge of a `412 CONFIRM_REQUIRED`, the statement of a `428 \
OPERATOR_SIGNATURE_REQUIRED`, `leaderUrl` of a `421 NOT_LEADER`). Each operation lists \
the codes it raises itself and those its credential implies, per status in \
`x-calimero-codes` (every error answer is the one `Error` schema); any operation may also answer \
`400 BAD_REQUEST`, `400 REJECTED`, `421 NOT_LEADER` and `503 UNAVAILABLE`. A route that \
takes a session may answer `421 WRONG_NODE` (no `details`): try another node. \
`x-calimero-details` on an error response maps each of its codes that carries `details` to \
that value's schema, and `x-calimero-error-details` at the root maps every such code: one \
code, one schema, so a client reads `details` by `error`.\n\n\
**Retries.** `x-calimero-retry-safe` marks the operations a client may send again after a \
`503` or a lost answer (`x-calimero-retry-requires` names the body field that makes it so, \
an idempotency key). The sealed channel's own `503` (`{ error: { code: \"busy\", message } }`, \
outside the envelope: too many sealed requests or sessions at once; the schema \
`x-calimero-sealed-refusal` names) carries `Retry-After: 1` on any sealed route; the node's \
`503 UNAVAILABLE` answers carry none. \
`x-calimero-step-up` (with `x-calimero-user-verification`, and `x-calimero-step-up-when` \
where it applies only sometimes) and `x-calimero-operator` mark \
the two-round operations below.\n\n\
**Numbers** that may exceed 2^53 (fuel, bytes) travel as decimal strings (D20); every \
other integer is `format: int64` (read it into a 64-bit integer), and stays below 2^53.";

/// The shared shapes.
fn components() -> Vec<Component> {
    vec![
        component(
            "Error",
            "An error answer: `error` is the code. `details`, absent for most codes, has the schema `x-calimero-details` names for the code on the operation's response (every code's: `x-calimero-error-details` at the document's root); a code it names no schema for carries none, or a value clients need not read.",
            "{ status: uint, error: string, message: string, details?: any }",
        ),
        component(
            "Confirm",
            "A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same request sent again with it approves exactly that request.",
            "{ challengeId: string, assertion: #Assertion }",
        ),
        component(
            "ConfirmRequired",
            "`details` of a `412 CONFIRM_REQUIRED`: the WebAuthn challenge to sign under `rpId` with one of the passkeys `allowCredentials` names (credential ids; empty: any of the account's), and the `challengeId` the retry's `confirm` names. `apps`: deleting a team that holds apps, the apps that go with it.",
            "{ challengeId: string, challenge: b64url, rpId: string, allowCredentials: string[], apps?: string[] }",
        ),
        component(
            "OperatorProof",
            "The operator key's Ed25519 signature (hex) over the statement a `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.",
            "{ nonce: string, signature: string }",
        ),
        component(
            "OperatorSignatureRequired",
            "`details` of a `428 OPERATOR_SIGNATURE_REQUIRED`: the statement to sign (`args` is the JSON text of its arguments, as signed) and the exact bytes to sign (`message`, standard base64).",
            "{ subject: string, method: string, args: string, nonce: string, cluster: string, message: string }",
        ),
        component(
            "Assertion",
            "The browser's answer to `navigator.credentials.get`, each field base64url.",
            "{ credentialId: string, authenticatorData: string, clientDataJson: string, signature: string }",
        ),
        component(
            "Registration",
            "The browser's answer to `navigator.credentials.create`, each field base64url.",
            "{ attestationObject: string, clientDataJson: string }",
        ),
        component(
            "Session",
            "A session: its token (15 minutes, by the issuing node) and the refresh token for `/auth/refresh`.",
            "{ accountId: string, sessionToken: string, expiresAt: uint, refreshToken: string }",
        ),
        component(
            "Applied",
            "An entry an app's log applied: its index, how it ended and the app's error (`null` when it ended `ok`; the app's own error value otherwise, free-form). `details` of a `422 UPGRADE_FAILED`.",
            r#"{ index: uint, status: "ok" | "appError" | "trap", error: any }"#,
        ),
        component(
            "NotLeader",
            "`details` of a `421 NOT_LEADER`: the leader's URL to send the same request to, and its node id where known.",
            "{ leaderUrl: string, leader?: uint }",
        ),
        component(
            "FeatureNotActive",
            "`details` of a `409 FEATURE_NOT_ACTIVE`: the feature the request needs, the cluster version it ships in (`needs`) and the version active now (`null`: none logged, and the nodes could not say).",
            "{ feature: string, needs: string, active: string | null }",
        ),
        component(
            "RuntimeTooOld",
            "`details` of a `409 RUNTIME_TOO_OLD`: the runtime the release needs and the replicas known to fall short of it, each with what it runs (a version, or `{ before }`: a release older than that one).",
            "{ minRuntimeVersion: string, nodes: { node: uint, runtime: string | { before: string } }[] }",
        ),
        component(
            "ReleaseDeadline",
            "`details` of a `409 RELEASE_DEADLINE`: the mero-tee release whose deadline passed, and the deadline (`after` RFC 3339, `afterSecs` Unix seconds). Approving that release (or a newer one) lifts it.",
            "{ release: string, after: string, afterSecs: uint }",
        ),
        component(
            "UpgradePending",
            "`details` of a `409 UPGRADE_PENDING`: the release every node must run first, and the nodes that run an older one (`release` `null`: one this node could not attest).",
            "{ release: string, nodes: { node: uint, release: string | null }[] }",
        ),
        component(
            "AppError",
            "`details` of a `422 APP_ERROR`: the app's error code, its fields (the app's own value, free-form: the object its ABI declares for the error, or the app's message as a string when that is not JSON), and the log index of the call (absent when the node refused it before it ran: `UnknownMethod`, `WrongKind`, `Forbidden`, `ConfirmNotAView`, `NotAView`, `ConfirmRequired`).",
            "{ index?: uint, code: string, fields: any }",
        ),
        component(
            "Trap",
            "`details` of a `422 TRAP`: the log index of the run, how it trapped, and the engine's account of it.",
            r#"{ index: uint, kind: "OutOfFuel" | "Timeout" | "Cancelled" | "Unavailable" | "OutOfMemory" | "StackOverflow" | "Fault" | "HostDenied", detail: string }"#,
        ),
        component(
            "InitFailed",
            "`details` of a `422 INIT_FAILED`: the app made, and the error its `init` ended with (the app log's, free-form).",
            "{ appId: string, error: any }",
        ),
        component(
            "RecoveryNotReady",
            "`details` of a `409 RECOVERY_NOT_READY`: when the recovery may finish (Unix seconds).",
            "{ readyAt: uint }",
        ),
        component(
            "UploadPosition",
            "`details` of a `409 PART_CONFLICT`, `409 PART_OUT_OF_ORDER` or `409 UPLOAD_INCOMPLETE`: the part the upload expects next.",
            "{ next: uint }",
        ),
        component(
            "RetryAfter",
            "`details` of a `503 TIMEOUT` or `503 FILES_UNAVAILABLE`: the seconds to wait before sending the same request (with the same idempotency key) again.",
            "{ retryAfter: uint }",
        ),
        component(
            "ServiceRefused",
            "`details` of a `400 UNSUPPORTED_BUNDLE`, `400 BAD_BUNDLE` or `400 COMPUTE_ABI_MISMATCH` refusing one service of a release: the service, and `compute: true` when it was that service's compute component. Absent when the refusal is of a raw bundle.",
            "{ service: string, compute?: bool }",
        ),
        component(
            "SealedRefusal",
            "The sealed channel's own refusal, outside the envelope and in the clear: not an `Error`. `busy` (`503`, with `Retry-After: 1`): too many sealed requests or sessions at once, send it again; the other codes (`400`, `malformed` and the like) say the envelope could not be opened.",
            "{ error: { code: string, message: string } }",
        ),
        component(
            "Receipt",
            "The answering node's signature (receipt key, hex) over `(app, index, SHA-256(JCS(result)), node)`.",
            "{ app: string, index: uint, resultHash: string, node: uint, key: string, signature: string }",
        ),
        component(
            "Access",
            "Who may reach an app at all.",
            r#"{ kind: "owner" } | { kind: "org" } | { kind: "public" } | { kind: "accounts", accounts: string[] } | { kind: "team", team: string }"#,
        )
        .of::<crate::types::Access>(),
        component(
            "OrgRole",
            "A member's role in an organization.",
            r#""guest" | "member" | "admin" | "owner""#,
        )
        .of::<crate::types::OrgRole>(),
        component(
            "TeamRole",
            "A member's standing in one team.",
            r#""lead" | "member" | "readOnly""#,
        )
        .of::<crate::types::TeamRole>(),
        component(
            "TeamJoin",
            "Open (`everyone` who reaches the parent) or restricted (`invite`) membership.",
            r#""everyone" | "invite""#,
        )
        .of::<crate::types::TeamJoin>(),
        component(
            "OutboundSelection",
            "`manifest` (every prefix the release declares) or these prefixes.",
            r#""manifest" | string[]"#,
        ),
        component(
            "Deployer",
            "An account or an organization.",
            "{ account: string } | { org: string }",
        )
        .of::<crate::system::Deployer>(),
        component(
            "SignUp",
            "The cluster's sign-up policy.",
            r#""open" | "invite" | "closed""#,
        ),
        component(
            "Page",
            "A page of a list: `next` is the `after` of the next page, `null` on the last.",
            "{ next: string | null }",
        ),
        component(
            "Release",
            "A mero-tee release.",
            "{ version: string, profile: string }",
        ),
        component(
            "AttestationDoc",
            "A node's attestation: the TDX quote (base64), the key it binds (hex) and what it was asked for. `receiptKey` and `governance` with `purpose=receipts`, `kms` with `purpose=kms` on a node with a KMS record, `clusterIdentity` with `purpose=orchestrator`; `jobKey` only from a job node, never from this API.",
            "{ quote: string, transportKey: string, appHash: string, nonce: string, release: #Release, cluster: string, node: uint, mock: bool, receiptKey?: string, governance?: #Governance, kms?: #KmsKeys, jobKey?: string, clusterIdentity?: string }",
        )
        .of::<calimero_client::attest::AttestationDoc>(),
        component(
            "KmsKeys",
            "What a node verified of the mero-kms replicas it took its keys from: the storage key `merod` fetched, and the data disk's key the boot script fetched; each absent without a record.",
            "{ storage?: #KmsEvidence, disk?: #KmsEvidence }",
        )
        .of::<calimero_client::attest::KmsKeys>(),
        component(
            "KmsEvidence",
            "The KMS quote a node accepted a key under: the KMS host, the mero-kms release (and image profile) whose signed policy it matched, the quote's registers (hex), its TCB status, SHA-256 and when it was verified (Unix seconds); `mock` (absent: false) in development builds only.",
            "{ kmsHost: string, release: string, profile?: string, mrtd: string, rtmr0: string, rtmr1: string, rtmr2: string, rtmr3: string, tcbStatus?: string, quoteSha256: string, verifiedAt: uint, mock?: bool }",
        )
        .of::<calimero_tee_attestation::KmsEvidence>(),
        component(
            "GroupStatus",
            "A Raft group's status as one node sees it: the leader (`null`: none known), whether this node is it, the term, the last log index (`null`: an empty log), the applied index, and the voters and learners.",
            "{ group: uint, leader: uint | null, is_leader: bool, term: uint, last_log: uint | null, applied: uint, voters: uint[], learners: uint[] }",
        ),
        component(
            "RollbackStatus",
            "A node's rollback counter: the cluster's epoch (rollbacks forced so far), its generation and mark, whether this start was forced (`--accept-rollback`), and the seconds since mero-kms last acknowledged a checkpoint.",
            "{ epoch: uint, generation: uint, mark: uint, forced: bool, sinceCheckpointSecs: uint }",
        ),
        component(
            "NodeRole",
            "What a node does: hold app replicas, or run compute jobs.",
            r#""replica" | "job""#,
        )
        .of::<crate::types::NodeRole>(),
        component(
            "NodeRecord",
            "A registered node: its id, address, zone, role and (a job node's) compute pool, absent for none.",
            "{ id: uint, addr: string, zone: string, role: #NodeRole, pool?: string }",
        )
        .of::<crate::types::NodeRecord>(),
        component(
            "Quotas",
            "The per-account limits, and the compute and file limits of each organization (or personal account) that has no override. `maxFuelPerDeveloperPerDay` (0: no limit) is a decimal string; the others are JSON numbers.",
            "{ maxAppsPerAccount: uint, maxBundleBytesPerAccount: uint, maxApiKeysPerAccount: uint, maxOrgsPerAccount: uint, maxAccounts: uint, maxFileBytesPerOrg: uint, maxConcurrentRunsPerOrg: uint, computeFuelPerOrg: uint, maxFuelPerDeveloperPerDay: u64 }",
        )
        .of::<crate::system::Quotas>(),
        component(
            "OrgQuotas",
            "One payer's limits in place of the cluster's; a limit left out is the cluster's.",
            "{ maxConcurrentRuns?: uint, computeFuel?: u64, maxFileBytes?: uint }",
        )
        .of::<crate::system::OrgQuotas>(),
        component(
            "QuotaSettings",
            "`GET /ops/quotas`: the cluster's limits and, under `orgs`, each payer's overrides by payer id.",
            "{ maxAppsPerAccount: uint, maxBundleBytesPerAccount: uint, maxApiKeysPerAccount: uint, maxOrgsPerAccount: uint, maxAccounts: uint, maxFileBytesPerOrg: uint, maxConcurrentRunsPerOrg: uint, computeFuelPerOrg: uint, maxFuelPerDeveloperPerDay: u64, orgs: { [string]: #OrgQuotas } }",
        )
        .of::<QuotaSettings>(),
        component(
            "UsageDay",
            "What an app's runs used on one UTC day (`date`, `YYYY-MM-DD`), each figure a decimal string; `wallMs` is information only.",
            "{ date: string, runs: u64, fuel: u64, wallMs: u64, writes: u64, bytesWritten: u64, events: u64 }",
        ),
        component(
            "RunFile",
            "A file a run reads or wrote: its id, and its plaintext size and SHA-256 (hex) only for a caller who may list the app's files.",
            "{ id: string, size?: uint, sha256?: string }",
        ),
        component(
            "Run",
            "A compute run's record, without its arguments or keys: its method, state, caps (`fuel` a JSON number here), inputs, attempts (`tsMs` Unix milliseconds) and final outcome (`null` while it runs; `message` the failure text, `null` unless the caller may list the app's files). `progress` is the leader's last poll (`null` elsewhere, when it is not active, or to a caller who may not list files); `queue` where the run waits for a job node, to a caller who may list files only.",
            r#"{ run: string, method: string, state: "queued" | "running" | "done" | "failed" | "cancelled", cancelRequested: bool, startTsMs: uint, caps: { fuel: uint, memoryMib: uint, wallSecs: uint }, inputs: #RunFile[], attempts: { attempt: uint, node: uint, tsMs: uint }[], final: { attempt: uint, outcome: { kind: "done" } | { kind: "failed", reason: "fuel" | "memory" | "trap" | "app-error" | "integrity" | "cancelled" | "lost" | "refused" | "timeout" | "quota" | "not-granted" | "too-large" } | { kind: "cancelled" }, outputs: #RunFile[], usage: { cpuMicros: uint, gibMicros: uint, fuel: uint }, tsMs: uint, message: string | null } | null, progress: { percent: uint, step: string } | null, queue?: { state: "unplaced" | "waiting" | "granted", capacity: uint, pool: string, reason?: "NO_CAPACITY" | "POOL_FULL" | "PAYER_AT_RUN_QUOTA" | "PAYER_AT_FUEL_QUOTA" } }"#,
        ),
        component(
            "AccountProof",
            "A statement, signed with the answering node's receipt key (hex, Ed25519 over the statement's canonical JSON after its domain), that the account asked to bind `nonce` for `audience`, valid from `issuedAt` to `expiresAt` (Unix seconds).",
            "{ statement: #AccountStatement, key: string, signature: string }",
        )
        .of::<calimero_client::attest::AccountProof>(),
        component(
            "AccountStatement",
            "What an account proof states: the cluster and node, the account, the nonce and audience it binds, and when it was issued and expires (Unix seconds).",
            "{ cluster: string, node: uint, account: string, nonce: string, audience: string, issuedAt: uint, expiresAt: uint }",
        )
        .of::<calimero_client::attest::AccountStatement>(),
        component(
            "PackageOrigin",
            "A custom frontend origin of a package: the rp-id its passkeys use, who registered it and when (RFC 3339), and whether the cluster serves the package's frontend there.",
            "{ package: string, origin: string, rpId: string, addedBy: string, addedAt: string, served: bool }",
        ),
        component(
            "SignUpState",
            "The sign-up policy and what it implies: whether organization invites and cluster sign-up invites admit a new account, and whether one is required.",
            "{ policy: #SignUp, orgInvites: bool, signUpInvites: bool, inviteRequired: bool }",
        ),
        component(
            "ApprovedReleases",
            "The mero-tee releases the scope approves now, after the change.",
            "{ approvedReleases: string[] }",
        ),
        component(
            "OutboundApproved",
            "The outbound approval the scope holds now for the package's publisher (`signer`), after the change.",
            "{ signer: string, approval: #OutboundApproval }",
        ),
        component(
            "Uninstalled",
            "The versions an uninstall removed.",
            "{ removed: string[] }",
        ),
        component(
            "Deadlines",
            "Every release deadline after the change, oldest release first (`after` RFC 3339, `afterSecs` Unix seconds).",
            "{ deadlines: { version: string, after: string, afterSecs: uint }[] }",
        ),
        component(
            "SignUpSet",
            "The cluster's governance and sign-up policy after the change.",
            "{ governance: #Governance, signUp: #SignUpState }",
        ),
        component(
            "InviteKey",
            "The key of the cluster sign-up invite withdrawn.",
            "{ inviteKey: string }",
        ),
        component(
            "UpgradeRun",
            "An organization-wide upgrade run: the apps it targets, those done, and why each failed one failed.",
            "{ runId: string, package: string, version: string, team?: string, cascade: bool, by: string, startedAt: uint, apps: string[], done: string[], failed: { [string]: any }, finished: bool }",
        )
        .of::<crate::types::UpgradeRun>(),
        component(
            "App",
            "An app instance (a context) as the directory records it. `executor` is absent for an SDK 0.12 app (wasmer), which this node lists but does not run; `release` is the package release it runs, absent for a record from before or a raw upload; `restore` is a restore from a backup under way.",
            r#"{ appId: string, name: string, group: uint, bundle: string, owner: string, org: string | null, access: #Access, confirmMethods: string[], executor?: "component" | "wasmer", replicas: uint[], createdAt: uint, initArgs: any, computeBundle?: string, metadata: any, lastUpgrade?: #Upgrade, deployment?: string, release?: #PackageRef, restore?: { seed?: uint, source?: { name: string, sha256: string, index: uint } } }"#,
        )
        .of::<crate::types::AppRecord>(),
        component(
            "AppEntry",
            "An app as `GET /apps` lists it: `package` and `version` name the release its bundle came from (`null` for a raw upload), `deployment` the deployment it was created from (`null` for none; absent from an older node, as 1.0.0-rc.1).",
            "{ appId: string, name: string, owner: string, org: string | null, bundle: string, package: string | null, version: string | null, deployment?: string | null }",
        ),
        component(
            "PackageRef",
            "One service of one package release: `service` is absent for a single-service bundle, `compute` (absent: false) marks the service's compute component.",
            "{ package: string, version: string, service?: string, compute?: bool }",
        )
        .of::<crate::system::PackageRef>(),
        component(
            "Upgrade",
            "The last upgrade tried on an app, done or failed: `from` and `to` are bundle hashes, `by` the account (or `operator`), `run` the organization-wide run it was part of, `apiKey` the upgrade key that made it.",
            "{ from: string, to: string, at: uint, by: string, ok: bool, error?: any, run?: string, apiKey?: string, fromRelease?: #PackageRef, toRelease?: #PackageRef }",
        )
        .of::<crate::types::UpgradeRecord>(),
        component(
            "Package",
            "A package and the releases of it the caller may see, oldest first; `latest` is the newest of those.",
            r#"{ package: string, signerId: string, owner: string, latest: string, versions: { version: string, mpk: string, size: uint, source: "registry" | "upload", services: { [string]: string }, metadata: any, outbound: string[], installedBy: string, installedAt: uint }[] }"#,
        ),
        component(
            "ApiKey",
            "An API key as the account's list shows it (never its secret): `apps` and `methods` are `null` for every one; `upgrades` makes it an upgrade key.",
            "{ keyId: string, account: string, label: string, apps: string[] | null, methods: string[] | null, readOnly: bool, allowConfirm: bool, expiresAt: uint, upgrades?: { app: string, package: string, signer: string, proofKey: string } }",
        )
        .of::<crate::types::ApiKeyRecord>(),
        component(
            "Org",
            "An organization as `GET /orgs/{org}` answers it. `members` maps each account to its role, `capabilities` and `defaultCapabilities` are capability bits, `outboundApprovals` is keyed by package signer, `upgradeRuns` by run id; `recovery` is absent when the organization set no recovery key, `allApps` when no member gets every app added later.",
            r#"{ orgId: string, name: string, members: { [string]: #OrgRole }, teams: { [string]: #Team }, defaultCapabilities: uint, capabilities: { [string]: uint }, approvedReleases: string[], outboundApprovals: { [string]: #OutboundApproval }, metadata: any, memberMetadata: { [string]: any }, upgradeRuns: { [string]: #UpgradeRun }, recovery?: { publicKey: string, setAt: uint, pending: { account: string, startedAt: uint, readyAt: uint } | null }, allApps?: string[], mode?: "legacy" | "standalone" | "shared", releaseApprovers?: string[], publishes?: string[], newWorkStopped?: { release: string, after: string } | null }"#,
        )
        .of::<OrgAnswer>(),
        component(
            "Team",
            "A team as its organization records it: `members` maps each direct row to whether it is a lead; `capabilities` are capability bits per row; `defaultCapabilities` is absent when unset.",
            "{ teamId: string, name: string, parent: string | null, join: #TeamJoin, members: { [string]: bool }, readOnly: string[], capabilities: { [string]: uint }, metadata: any, removed: string[], defaultCapabilities?: uint }",
        )
        .of::<crate::types::Team>(),
        component(
            "TeamView",
            "A team as `GET /orgs/{org}/teams/{team}` answers it: each member's row, the child teams the caller sees, its apps, and the caller's own row (`myRole`, `null` for none) and membership (`member`). `package` names the package of an app team; it and `defaultCapabilities` are absent from an older node (1.0.0-rc.1 sends neither).",
            "{ teamId: string, name: string, parent: string | null, join: #TeamJoin, metadata: any, package?: string | null, defaultCapabilities?: uint | null, members: { account: string, role: #TeamRole, capabilities: uint }[], children: string[], apps: { appId: string, name: string, contextId: string }[], myRole: #TeamRole | null, member: bool }",
        ),
        component(
            "Deployment",
            "A deployment as its managers list it. `host` is its frontend host, `null` on a node with no apps domain; times are RFC 3339.",
            "{ id: string, host: string | null, name: string, owner: #Deployer, apps: { app: string, package: string, version: string, addedBy: string, addedAt: string }[], createdBy: string, createdAt: string }",
        ),
        component(
            "DeadLetter",
            "A cross-app call that was never delivered: its sequence number, the target app and method, why, and when (`atMs`, Unix milliseconds).",
            "{ seq: uint, app: string, method: string, reason: string, atMs: uint }",
        )
        .of::<crate::component::xcall::DeadLetter>(),
        component(
            "Governance",
            "Who decides what on this cluster: release approvals, app upgrades and outbound approvals (`owner`: each app's owner; `operator`: the operator), and, left out at their defaults, `deployments` (`open`), `deployers` (`operator`) and `signUp` (`open`).",
            r#"{ releases: "owner" | "operator", appUpgrades: "owner" | "operator", outbound: "owner" | "operator", deployments?: "granted" | "open", deployers?: "owner" | "operator", signUp?: "invite" | "closed" | "open" }"#,
        )
        .of::<crate::system::Governance>(),
        component(
            "OutboundApproval",
            "What an owner approved for one publisher's apps to call out to: the approved URL prefixes, and every prefix it was shown.",
            "{ prefixes: string[], reviewed: string[] }",
        )
        .of::<crate::types::OutboundApproval>(),
        component(
            "AppScope",
            "The scope whose approvals cover an app: its organization, or its owner's account (never named).",
            r#"{ kind: "org", org: string } | { kind: "account" }"#,
        ),
        component(
            "ClusterRelease",
            "A mero-tee release the cluster's nodes run: how many run it, and whether the scope approved it (`true` where nothing was ever recorded).",
            "{ release: string, nodes: uint, approved: bool }",
        ),
        component(
            "BundleRelease",
            "A bundle by its digest, with the release it came from where the reader may see it.",
            "{ bundle: string, package?: string, version?: string }",
        ),
        component(
            "ReportUpgrade",
            "An app's last upgrade as a report shows it. `actor`, `error`, `run` and `apiKey` are shown to the app's people, never to the operator.",
            r#"{ from: #BundleRelease, to: #BundleRelease, at: uint, ok: bool, actorKind: "owner" | "member" | "operator", actor?: string, error?: any, run?: string, apiKey?: string }"#,
        ),
        component(
            "ReportApp",
            "One app in a release report (`GET /auth/apps`, `GET /orgs/{org}/apps`, `GET /ops/apps`): its bundle and release (`package`, `null` where the reader may not see it), the publisher's key and bound organization, who holds its upgrades and release approvals, its last upgrade, the releases approved to hold it (`null`: unrestricted), each replica's release, and `lastWriteAt` (Unix seconds, `null` when unknown). `name`, `team`, `availableVersions` (newer releases, oldest first), `latestReachable` and `latestBlocked` (the newest release and why an upgrade would not take it) are for the app's people, never the operator.",
            r#"{ app: string, group: uint, scope: #AppScope, bundle: string, package: { package: string, version: string, service?: string | null } | null, publisher: { signer: string, org: string | null } | null, upgradesHeldBy: "operator" | "owner" | "publisher", releasesApprovedBy: "operator" | "publisher" | "owner", lastUpgrade: #ReportUpgrade | null, approvedReleases: string[] | null, replicas: { node: uint, release: string | null, approved: bool }[], lastWriteAt: uint | null, name?: string, team?: string, availableVersions?: string[], latestReachable?: string | null, latestBlocked?: { version: string, reason: "authority" | "runtime", message: string, minRuntimeVersion?: string, nodes?: { node: uint, runtime: string | { before: string } | null }[] } }"#,
        ),
        component(
            "DeploymentRecord",
            "A deployment as the directory records it: the release it offers by app name (`addedAt` Unix seconds), who created it and when (Unix seconds).",
            "{ id: string, name: string, owner: #Deployer, apps: { [string]: { package: string, version: string, addedBy: string, addedAt: uint } }, createdBy: string, createdAt: uint }",
        )
        .of::<crate::system::Deployment>(),
        component(
            "DeployerGrant",
            "A grant to install where deployments are granted: until when (Unix seconds; absent: it does not end), when and by whom it was granted.",
            "{ until?: uint, grantedAt: uint, grantedBy: string }",
        )
        .of::<crate::system::DeployerGrant>(),
        component(
            "SignUpInvite",
            "A cluster sign-up invite, never its token: uses left, when it expires (`expiresAt` RFC 3339, `expiresAtSecs` Unix seconds), who made it and when (RFC 3339).",
            "{ inviteKey: string, uses: uint, expiresAt: string, expiresAtSecs: uint, createdBy: string, createdAt: string }",
        ),
        component(
            "ProofOnly",
            "What an operator decision that names nothing in its body may carry: none at first, the operator's proof after the `428`.",
            "{ operatorProof?: #OperatorProof }",
        )
        .closed_body(),
        component(
            "ConfirmOnly",
            "The body of a step-up that names nothing else: none (or `{}`) at first, the passkey assertion after the `412`.",
            "{ confirm?: #Confirm }",
        ),
        component(
            "QuotaChanges",
            "The limits `POST /ops/quotas` changes, each optional: without `org`, the cluster's (those `GET /ops/quotas` answers, `maxFuelPerDeveloperPerDay` a decimal string); with `org`, one payer's (`maxConcurrentRuns`, `computeFuel` a decimal string, `maxFileBytes`; `null` clears one). `orgs` is ignored. Any other property is refused `400 UNKNOWN_QUOTA`.",
            "{ org?: string, operatorProof?: #OperatorProof, orgs?: any, maxAppsPerAccount?: uint, maxBundleBytesPerAccount?: uint, maxApiKeysPerAccount?: uint, maxOrgsPerAccount?: uint, maxAccounts?: uint, maxFileBytesPerOrg?: uint, maxConcurrentRunsPerOrg?: uint, computeFuelPerOrg?: uint, maxFuelPerDeveloperPerDay?: u64, maxConcurrentRuns?: uint | null, computeFuel?: u64 | null, maxFileBytes?: uint | null }",
        )
        .closed_body(),
    ]
}

/// `GET /orgs/{org}`'s answer, for the checks: the organization as the directory records it,
/// and the fields `org_get` adds to it (`newWorkStopped` from a `system::Stall`, `after` as
/// RFC 3339).
#[derive(Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrgAnswer {
    #[serde(flatten)]
    org: crate::types::Org,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<crate::types::OrgMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    release_approvers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    publishes: Option<Vec<String>>,
    /// Absent from an older node; `null` when nothing stops new work.
    #[serde(
        default,
        deserialize_with = "crate::types::present",
        skip_serializing_if = "Option::is_none"
    )]
    new_work_stopped: Option<Option<StallAnswer>>,
}

#[derive(Serialize, serde::Deserialize)]
struct StallAnswer {
    release: String,
    after: String,
}

/// `GET /ops/quotas`'s answer, for the checks: the cluster's limits and the per-payer
/// overrides `ops_quotas_get` adds under `orgs`.
#[derive(Serialize, serde::Deserialize)]
struct QuotaSettings {
    #[serde(flatten)]
    quotas: crate::system::Quotas,
    orgs: BTreeMap<String, crate::system::OrgQuotas>,
}

/// The registry: every public, auth and ops route of [`super::router`].
#[allow(
    clippy::too_many_lines,
    reason = "one entry per route, in the router's order"
)]
fn operations() -> Vec<Op> {
    use Who::{AccountHolder, Anyone, AppAccess, Installer, Ops, Session, SignedIn};

    const PAGE: &str = "{ after?: string, limit?: uint }";
    const APPROVAL: &str = "{ version: string, approved?: bool, confirm?: #Confirm }";
    const OUTBOUND: &str =
        "{ package: string, prefixes?: #OutboundSelection, replace?: bool, confirm?: #Confirm }";
    vec![
        // ---------------------------------------------------------------- public
        get("/health", "health", Anyone, "Node health")
            .about("In the clear. The node's id, the system group's leader (`null`: none known), its `merod` release, the newest cluster version it supports (`clusterVersion`) and the cluster's rollback epoch (`null` on a node without a rollback counter).")
            .ok("{ ok: bool, node: uint, systemLeader: uint | null, release: string, clusterVersion: string, rollbackEpoch: uint | null }"),
        get("/attestation", "attestation", Anyone, "The node's attestation")
            .about("In the clear. A TDX quote binding the node's transport key (or, with `purpose=receipts`, its receipt key and the governance; with `purpose=kms`, what it verified of its KMS; with `purpose=orchestrator`, the cluster identity this replica holds) to `nonce`; `app`/`appHash` bind an app's bundle hash. A client checks it before it seals anything to the key.")
            .query::<super::AttestQuery>(r#"{ nonce: string, app?: string, appHash?: string, purpose?: "receipts" | "kms" | "orchestrator" }"#)
            .ok("#AttestationDoc")
            .errors(&[(400, "BAD_NONCE"), (400, "BAD_HASH"), (404, "NOT_FOUND")]),
        get("/releases", "releases_public", Anyone, "The cluster's releases")
            .about("The cluster's governance and the deadlines set on mero-tee releases, oldest release first: after `after` (RFC 3339; `afterSecs` Unix seconds) a scope that has not approved that release (or a newer one) starts no new work.")
            .ok("{ governance: #Governance, deadlines: { version: string, after: string, afterSecs: uint }[] }"),
        get("/publishers/{signer}", "publisher_binding", SignedIn, "Whether a signing key is bound")
            .about("Whether a package signing key (`did:key`) is bound to an organization on this cluster (`bound`, never which), and its binding generation (`0`: never bound).")
            .ok("{ signerId: string, cluster: string, generation: uint, bound: bool }"),
        get("/sign-up", "sign_up_public", Anyone, "The sign-up policy")
            .about("Who may create an account here, the cluster's name and the tag a sign-in uses to pick the person's passkey for it.")
            .ok("{ policy: #SignUp, orgInvites: bool, signUpInvites: bool, inviteRequired: bool, cluster: string, passkeyTag: string }"),
        // ---------------------------------------------------------------- accounts
        post("/auth/passkey/register/begin", "register_begin", Anyone, "Start a sign-up")
            .about("Creation options for a new passkey. `invite` is an organization or cluster sign-up invite token, which the policy may require; `403 SIGN_UP_REFUSED` when the policy would refuse the sign-up. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their shape is WebAuthn's.")
            .json::<super::RegisterBegin>("{ name?: string, origin?: string, invite?: string }")
            .ok("{ challengeId: string, options: any }")
            .errors(&[(403, "SIGN_UP_REFUSED"), (429, "RATE_LIMITED")]),
        post("/auth/passkey/register/finish", "register_finish", Anyone, "Finish a sign-up")
            .about("The new passkey's registration. Answers `412 CONFIRM_REQUIRED` first: the new passkey signs the account it creates; the retry with `confirm` (and the same `invite`) opens a session, and redeems `invite` for the new account.")
            .json::<super::RegisterFinish>("{ challengeId: string, registration: #Registration, invite?: string, confirm?: #Confirm }")
            .ok("#Session")
            .step_up_uv_preferred()
            .errors(&[(400, "BAD_REGISTRATION"), (400, "WRONG_CHALLENGE"), (401, "PASSKEY_UNKNOWN_HERE"), (403, "SIGN_UP_REFUSED"), (421, "WRONG_NODE")]),
        post("/auth/passkey/login/begin", "login_begin", Anyone, "Start a sign-in")
            .about("A WebAuthn challenge for any passkey of the cluster. `origin` is an app frontend's registered origin, whose rp-id the ceremony then uses. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their shape is WebAuthn's.")
            .opt_json::<super::LoginBegin>("{ origin?: string }")
            .ok("{ challengeId: string, options: any }")
            .errors(&[(429, "RATE_LIMITED")]),
        post("/auth/passkey/login/finish", "login_finish", Anyone, "Finish a sign-in")
            .json::<super::AssertionBody>("{ challengeId: string, assertion: #Assertion }")
            .ok("#Session")
            .errors(&[(400, "WRONG_CHALLENGE"), (400, "UNKNOWN_CHALLENGE"), (400, "EXPIRED_CHALLENGE"), (401, "PASSKEY_UNKNOWN_HERE"), (421, "WRONG_NODE")]),
        post("/auth/passkeys/add/begin", "passkey_add_begin", AccountHolder, "Start adding a passkey")
            .about("Creation options for another passkey (a device) on the caller's account. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their shape is WebAuthn's.")
            .ok("{ challengeId: string, options: any }"),
        post("/auth/passkeys/add/finish", "passkey_add_finish", AccountHolder, "Finish adding a passkey")
            .about("Answers `412` first; sent again with `confirm` signed by a passkey the account already has.")
            .json::<super::AddPasskeyFinish>("{ challengeId: string, registration: #Registration, confirm?: #Confirm }")
            .ok("{ credentialId: string }")
            .step_up()
            .errors(&[(400, "BAD_REGISTRATION"), (400, "WRONG_CHALLENGE")]),
        post("/auth/passkeys/{cred}", "passkey_label", AccountHolder, "Name a passkey")
            .json::<super::PasskeyLabelBody>("{ label: string }")
            .ok("{}"),
        delete("/auth/passkeys/{cred}", "passkey_remove", AccountHolder, "Remove a passkey")
            .about("Its sessions end. `412` first; the retry's JSON body is `{ confirm }`.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up(),
        post("/auth/recovery", "recovery_create", AccountHolder, "Make a recovery code")
            .about("Shown once, replacing any; `412` first.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{ code: string }")
            .step_up()
            .errors(&[(400, "WRONG_CHALLENGE")]),
        delete("/auth/recovery", "recovery_remove", AccountHolder, "Remove the recovery code")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up(),
        post("/auth/recovery/begin", "account_recovery_begin", Anyone, "Start an account recovery")
            .about("Signed out, with the recovery code: creation options for the passkey that will replace the lost ones. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their shape is WebAuthn's.")
            .json::<super::RecoveryBegin>("{ code: string }")
            .ok("{ challengeId: string, options: any }")
            .errors(&[(421, "WRONG_NODE"), (429, "RATE_LIMITED")]),
        post("/auth/recovery/start", "account_recovery_start", Anyone, "Spend the recovery code")
            .about("Logs the new passkey as pending; it may finish the recovery once `readyAt` passed (72 h). `401` for a wrong or spent code.")
            .json::<super::RecoveryStart>("{ challengeId: string, code: string, registration: #Registration }")
            .ok("{ accountId: string, credentialId: string, startedAt: uint, readyAt: uint }")
            .errors(&[(400, "BAD_REGISTRATION"), (400, "WRONG_CHALLENGE"), (401, "UNAUTHENTICATED"), (409, "RECOVERY_PENDING")]),
        post("/auth/recovery/cancel", "account_recovery_cancel", AccountHolder, "Cancel an account recovery")
            .about("With a passkey the account has; `412` first.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up(),
        post("/auth/recovery/finish", "account_recovery_finish", Anyone, "Finish an account recovery")
            .about("Approved by the new passkey once `readyAt` passed (`412` first): adds it and ends every session of the account.")
            .json::<super::RecoveryFinish>("{ accountId?: string, confirm?: #Confirm }")
            .ok("#Session")
            .step_up()
            .errors(&[(400, "WRONG_CHALLENGE"), (401, "UNAUTHENTICATED"), (409, "RECOVERY_NOT_READY"), (421, "WRONG_NODE")]),
        post("/auth/refresh", "refresh", Anyone, "Refresh a session")
            .about("A new session and the refresh token to use next (the same one until it is an hour old). A reused, superseded token ends the sessions it opened.")
            .json::<super::RefreshBody>("{ refreshToken: string }")
            .ok("#Session")
            .errors(&[(401, "UNAUTHENTICATED"), (421, "WRONG_NODE"), (503, "UNAVAILABLE")]),
        post("/auth/logout", "logout", Session, "End this session").ok("{}"),
        post("/auth/logout-all", "logout_all", AccountHolder, "End every session of the account")
            .about("`revoked`: how many sessions ended.")
            .ok("{ revoked: uint }"),
        get("/auth/sessions", "sessions_list", AccountHolder, "The account's live sessions")
            .about("Latest active first, at most 256. `openedAt` (Unix seconds) is `null` for a session opened before the node kept it.")
            .ok("{ sessions: { session: string, passkey: string | null, app: string | null, origin: string | null, openedAt: uint | null, lastActiveAt: uint, current: bool }[] }"),
        delete("/auth/sessions/{session}", "session_revoke", AccountHolder, "End one session")
            .about("`404` for a session the account does not hold live.")
            .ok("{}")
            .errors(&[(404, "NOT_FOUND")]),
        get("/auth/activity", "activity_list", AccountHolder, "What the account's passkeys did")
            .about("Newest first, the last 100: sign-ins (`app`, `origin`) and approvals (`method`, `subject`); `after` keeps the entries after that Unix second.")
            .query::<super::ActivityQuery>("{ after?: uint }")
            .ok(r#"{ activity: { at: uint, passkey: string, kind: "signIn" | "approval", app?: string | null, origin?: string | null, method?: string, subject?: string }[] }"#),
        post("/auth/releases", "account_release_approval", AccountHolder, "Approve a mero-tee release for the account's apps")
            .about("Where the owner holds release approvals; `412` first.")
            .json::<super::ReleaseApprovalBody>(APPROVAL)
            .ok("#ApprovedReleases")
            .step_up(),
        post("/auth/outbound", "account_outbound_approval", AccountHolder, "Approve outbound prefixes for the account's apps")
            .json::<super::OutboundApprovalBody>(OUTBOUND)
            .ok("#OutboundApproved")
            .step_up(),
        get("/auth/me", "me", SignedIn, "The caller's account")
            .about("The account, its passkeys (with `addedAt` and `lastSignInAt`), organizations, approvals and recovery state, and how the caller came (`via`).")
            .ok(r#"{ accountId: string, name: string, passkeys: { id: string, signCount: uint, label: string | null, addedAt?: uint | null, lastSignInAt?: uint | null }[], orgs: { orgId: string, name: string, role: #OrgRole }[], via: "session" | "apiKey", passkeyId: string | null, app?: string | null, origin?: string | null, approvedReleases: string[], outboundApprovals: { [string]: #OutboundApproval }, recoveryCode?: bool, recoveryPending?: { accountId: string, credentialId: string, startedAt: uint, readyAt: uint } | null }"#)
            .errors(&[(404, "NOT_FOUND")]),
        post("/auth/account-proof", "account_proof::prove", Session, "Prove the account to another party")
            .about("A statement signed with this node's receipt key that the caller's passkey session asked to bind `nonce` for `audience`.")
            .json::<super::account_proof::ProofBody>("{ nonce: string, audience: string }")
            .ok("#AccountProof")
            .errors(&[(400, "BAD_AUDIENCE"), (400, "BAD_NONCE")]),
        get("/auth/apps", "account_apps", SignedIn, "The account's own apps")
            .about("Outside any organization (a package-bound session: its package's): each app's release, the newer releases it may upgrade to, its last upgrade, its replicas' releases and `lastWriteAt`.")
            .query::<super::PageQuery>(PAGE)
            .ok("{ scope: #AppScope, governance: #Governance, approvedReleases: string[] | null, clusterReleases: #ClusterRelease[], apps: #ReportApp[], next: string | null }"),
        get("/auth/usage", "usage", SignedIn, "The account's usage against its quotas")
            .about("The apps the account owns, its bundle bytes (a decimal string), API keys and owned organizations; the file bytes held by each payer it belongs to (each organization it is in, and `account:<id>` for its own apps); the cluster's quotas; and what today's runs of the apps the account develops used (decimal strings).")
            .ok("{ apps: uint, bundleBytes: u64, apiKeys: uint, orgsOwned: uint, fileBytes: { [string]: uint }, quotas: #Quotas, developerFuelToday: u64, developerWritesToday: u64 }"),
        get("/auth/aliases", "alias_list", SignedIn, "The account's aliases").ok("{ aliases: { [string]: string } }"),
        get("/auth/aliases/{alias}", "alias_get", SignedIn, "Resolve an alias")
            .ok("{ alias: string, appId: string }")
            .errors(&[(404, "NOT_FOUND")]),
        put("/auth/aliases/{alias}", "alias_set", SignedIn, "Set an alias")
            .json::<super::AliasBody>("{ app: string }")
            .ok("{}")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/auth/aliases/{alias}", "alias_delete", SignedIn, "Remove an alias").ok("{}"),
        post("/auth/apikeys", "apikey_create", AccountHolder, "Make an API key")
            .about("Scoped to `apps` and `methods`, read-only or not, for `ttlSecs`; `upgrades` makes an upgrade key, which signs upgrades of one app in place of a passkey. `412` first. The key is shown once.")
            .json::<super::ApiKeyBody>("{ label?: string, apps?: string[], methods?: string[], readOnly?: bool, allowConfirm?: bool, ttlSecs?: uint, upgrades?: { app: string, package?: string }, confirm?: #Confirm }")
            .ok("{ keyId: string, key: string }")
            .step_up()
            .errors(&[(400, "BAD_UPGRADE_KEY"), (400, "WRONG_CHALLENGE"), (429, "QUOTA_EXCEEDED")]),
        get("/auth/apikeys", "apikey_list", AccountHolder, "The account's API keys").ok("{ keys: #ApiKey[] }"),
        delete("/auth/apikeys/{key}", "apikey_revoke", AccountHolder, "Revoke an API key")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        // ---------------------------------------------------------------- packages
        post("/bundles", "bundle_put", Installer, "Upload a raw component")
            .about("A `calimero:app` component, 16 MiB at most; only on a node started with `--allow-unsigned-bundles` or `--dev`. A WASM core module (SDK 0.12) is refused `400 UNSUPPORTED_BUNDLE`.")
            .raw("application/wasm", "the component")
            .ok("{ hash: string }")
            .errors(&[(400, "BAD_BUNDLE"), (400, "UNSUPPORTED_BUNDLE"), (403, "FORBIDDEN"), (413, "TOO_LARGE"), (429, "QUOTA_EXCEEDED")]),
        post("/packages", "package_upload", Installer, "Upload a signed release")
            .about("A signed `.mpk`, checked as an install from the registry is (signature, artifact digests, `minRuntimeVersion`, the package's pinned signer, every service compiles). Only with `--dev`; otherwise `403 UPLOADS_DISABLED`.")
            .raw("application/octet-stream", "the `.mpk`")
            .ok("{ package: string, version: string, signerId: string, services: { [string]: string } }")
            .errors(&[(400, "BAD_PACKAGE"), (400, "BAD_BUNDLE"), (400, "COMPUTE_ABI_MISMATCH"), (400, "UNSUPPORTED_BUNDLE"), (403, "UPLOADS_DISABLED"), (403, "DEPLOY_NOT_GRANTED"), (413, "TOO_LARGE")]),
        get("/packages", "package_list", SignedIn, "Installed packages")
            .about("Only the releases the caller may see.")
            .ok("{ packages: #Package[] }"),
        post("/packages/install", "package_install", Installer, "Install a release from the registry")
            .json::<super::InstallBody>("{ package: string, version: string }")
            .ok("{ package: string, version: string, signerId: string, services: { [string]: string } }")
            .errors(&[(400, "BAD_PACKAGE"), (400, "BAD_BUNDLE"), (400, "COMPUTE_ABI_MISMATCH"), (400, "UNSUPPORTED_BUNDLE"), (400, "NO_REGISTRY"), (403, "DEPLOY_NOT_GRANTED")]),
        get("/packages/{package}", "package_get", SignedIn, "One package")
            .about("The versions the caller may see; `404` when none.")
            .ok("#Package")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/packages/{package}", "package_uninstall", AccountHolder, "Uninstall a package")
            .about("Every version; refused while an app runs one.")
            .ok("#Uninstalled"),
        delete("/packages/{package}/{version}", "package_version_uninstall", AccountHolder, "Uninstall one version")
            .ok("#Uninstalled"),
        get("/package-origins/{package}", "origins::list", SignedIn, "A package's frontend origins")
            .about("The custom origins (and rp-ids) the package's manager registered for its frontend.")
            .ok("{ package: string, origins: #PackageOrigin[] }"),
        post("/package-origins/{package}", "origins::add", AccountHolder, "Register a frontend origin")
            .about("By whoever manages the package, with a passkey step-up over the change. Answers the origin as registered (the one already there, when the same origin is registered again).")
            .json::<super::origins::AddOriginBody>("{ origin: string, rpId?: string, served?: bool, confirm?: #Confirm }")
            .ok("#PackageOrigin")
            .step_up()
            .errors(&[(400, "BAD_RP_ID"), (429, "QUOTA_EXCEEDED")]),
        delete("/package-origins/{package}", "origins::remove", AccountHolder, "Remove a frontend origin")
            .json::<super::origins::RemoveOriginBody>("{ origin: string, confirm?: #Confirm }")
            .ok("{ package: string, origin: string, removed: bool }")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        // ---------------------------------------------------------------- apps
        post("/apps", "app_create", Installer, "Create an app instance")
            .about("From a release (`package`, `version`, `service`), a deployment's offer (`deployment`), or an uploaded `bundle`; in an organization (`org`) or the caller's own. `initArgs` go to the app's `init` and `metadata` is kept as given (both free-form JSON). Answers once `init` committed; `compute` says whether the app has a compute component.")
            .json::<super::CreateAppBody>("{ name?: string, bundle?: string, package?: string, version?: string, service?: string, org?: string, access?: #Access, initArgs?: any, metadata?: any, deployment?: string }")
            .ok("{ appId: string, contextId: string, group: uint, replicas: uint[], compute: bool }")
            .errors(&[(400, "BAD_REQUEST"), (400, "NOT_OFFERED"), (400, "VERSION_REQUIRED"), (400, "NO_SUCH_SERVICE"), (400, "AMBIGUOUS_COMPUTE"), (409, "RELEASE_DEADLINE"), (409, "RUNTIME_TOO_OLD"), (422, "INIT_FAILED"), (429, "QUOTA_EXCEEDED"), (503, "UNAVAILABLE")]),
        get("/apps", "app_list", SignedIn, "Apps the caller reaches")
            .about("Anyone may list public apps.")
            .query::<super::AppListQuery>("{ package?: string, bundle?: string }")
            .ok("{ apps: #AppEntry[] }"),
        get("/apps/{app}", "app_get", SignedIn, "One app")
            .about("`{app}` is an app id or its context id (`hex(SHA-256(app_id))`), here and on every `/apps/{app}` route.")
            .ok("#App")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/apps/{app}", "app_delete", Session, "Delete an app")
            .about("The owner or an organization admin.")
            .ok("{ appId: string, deleted: bool }"),
        post("/apps/{app}/metadata", "app_metadata", SignedIn, "Set an app's metadata")
            .about("The owner or an organization admin; 16 KiB at most. `metadata` is the caller's own JSON, free-form, kept as given.")
            .json::<super::MetadataBody>("{ metadata: any }")
            .ok("{}")
            .errors(&[(413, "TOO_LARGE")]),
        get("/apps/{app}/storage", "app_storage", AppAccess, "What an app stores")
            .about("Keys, bytes and blobs held, and `lastWriteAt` (Unix seconds of its last committed write, `null` before one).")
            .ok("{ keys: uint, bytes: uint, blobs: uint, blobBytes: uint, lastWriteAt?: uint | null }"),
        get("/apps/{app}/usage", "app_usage", SignedIn, "An app's runs per day")
            .about("The owner or an organization admin: `today` and the 30 `days` before it, figures as decimal strings; `wallMs` is information only.")
            .ok("{ today: #UsageDay, days: #UsageDay[] }")
            .errors(&[(403, "FORBIDDEN")]),
        get("/apps/{app}/abi", "app_abi", AppAccess, "An app's ABI")
            .about("The ABI its bundle embeds: methods, events, errors and state. `abi` is the bundle's own document, free-form here: its format is the ABI's, not this API's.")
            .ok("{ bundle: string, abi: any }"),
        get("/apps/{app}/openapi.json", "app_openapi", AppAccess, "An app's OpenAPI document")
            .about("Generated from the ABI of the bundle the app runs, under the access `/abi` has: one `POST /apps/{app}/methods/{method}` per method, with its arguments, result and errors. Free-form here: it is a document of its own, which a client reads as OpenAPI.")
            .ok("{ [string]: any }")
            .errors(&[(404, "NOT_FOUND"), (410, "UNSUPPORTED_ENGINE"), (422, "BAD_ABI")]),
        get("/apps/{app}/runs", "app_runs", AppAccess, "An app's compute runs")
            .about("Newest first, `limit` (1 to 100, default 20) at a time; `next` is the `before` of the next page, `null` on the last.")
            .query::<super::RunsQuery>("{ limit?: string, before?: string }")
            .ok("{ runs: #Run[], next: string | null }")
            .errors(&[(400, "BAD_CURSOR"), (400, "BAD_LIMIT"), (404, "NOT_FOUND")]),
        get("/apps/{app}/runs/{run}", "app_run", AppAccess, "One compute run")
            .about("Its record without arguments or keys: method, state, `cancelRequested`, `startTsMs`, attempts, input and output ids (sizes and SHA-256 only for callers who may list the app's files), usage, and `progress` as the leader last polled it.")
            .ok("#Run")
            .errors(&[(404, "NOT_FOUND")]),
        get("/apps/{app}/presence", "presence_get", AppAccess, "Who is present")
            .about("One entry per present device (a session's passkey, or an API key): its account, device and state (hex).")
            .ok("{ present: { account: string, device: string, state: string | null }[] }"),
        post("/apps/{app}/presence", "presence_publish", AppAccess, "Publish presence")
            .about("The caller's presence state (hex), in sequence; not logged. A `null` (or absent) `state` retracts it.")
            .json::<super::PresenceBody>("{ seq: uint, state?: string | null }")
            .ok("{}")
            .errors(&[(400, "BAD_STATE"), (413, "TOO_LARGE")]),
        get("/apps/{app}/presence/stream", "presence_stream", AppAccess, "Presence as server-sent events")
            .events("a `snapshot`, then `presence` diffs"),
        post("/apps/{app}/intents", "intents", AppAccess, "Call a method")
            .about("A logged call: the result, the events of the call the caller receives, and the answering node's receipt. A `confirm` method answers `412` first. An app error is `422 APP_ERROR` with `details { index, code, fields }`; a trap `422 TRAP`. `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them.")
            .json::<super::IntentBody>("{ method: string, args?: any, idempotencyKey?: string, confirm?: #Confirm }")
            .ok("{ index: uint, result: any, events: { kind: string, data: any }[], receipt: #Receipt }")
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "RELEASE_DEADLINE"), (412, "CONFIRM_REQUIRED"), (413, "ARGS_TOO_LARGE"), (422, "APP_ERROR"), (422, "TRAP"), (429, "QUOTA_EXCEEDED"), (503, "TIMEOUT"), (503, "UNAVAILABLE")])
            .step_up_when("the method is a `confirm` method of the app's ABI")
            .retry_safe_with("idempotencyKey"),
        get("/apps/{app}/attestation", "app_attestation", AppAccess, "Every replica's receipt-key attestation")
            .about("Each replica's `/attestation?purpose=receipts` for `nonce` (32 bytes, hex), or why it could not be had (`error`). Any other query parameter is ignored.")
            .query::<super::AppAttestQuery>("{ nonce: string }")
            .ok("{ appId: string, replicas: ({ node: uint, attestation: #AttestationDoc } | { node: uint, error: string })[] }"),
        post("/apps/{app}/views", "views", AppAccess, "Read with a view method")
            .about("Read on a caught-up replica (`consistency: \"strong\"`, the default; any value but `\"stale\"` reads as it), or (`\"stale\"`) on the one answering; `minIndex` waits until the replica applied it (`503 NOT_CAUGHT_UP`). `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them.")
            .json::<super::ViewBody>(r#"{ method: string, args?: any, consistency?: "strong" | "stale", minIndex?: uint }"#)
            .ok("{ index: uint, result: any, receipt: #Receipt }")
            .errors(&[(422, "APP_ERROR"), (422, "TRAP"), (503, "NOT_CAUGHT_UP"), (503, "TIMEOUT")])
            .retry_safe(),
        post("/apps/{app}/methods/{method}", "app_method", AppAccess, "Call a method by its path")
            .about("The body of `/intents` or `/views` without `method`, which the path names, as the app's OpenAPI document lists it: a view the running bundle's ABI declares (without `confirm`) is answered as `/views`, anything else as `/intents`, with that route's gate, step-up and limits. Retry-safe as a view always, as a call with `idempotencyKey`. `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them.")
            .json::<super::MethodBody>(r#"{ args?: any, idempotencyKey?: string, confirm?: #Confirm, consistency?: "strong" | "stale", minIndex?: uint }"#)
            .ok("{ index: uint, result: any, events?: { kind: string, data: any }[], receipt: #Receipt }")
            .errors(&[(400, "RESERVED_METHOD"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "RELEASE_DEADLINE"), (412, "CONFIRM_REQUIRED"), (413, "ARGS_TOO_LARGE"), (422, "APP_ERROR"), (422, "TRAP"), (429, "QUOTA_EXCEEDED"), (503, "NOT_CAUGHT_UP"), (503, "TIMEOUT"), (503, "UNAVAILABLE")])
            .step_up_when("the method is a `confirm` method of the app's ABI")
            .retry_safe_with("idempotencyKey"),
        get("/apps/{app}/events", "events", AppAccess, "An app's events as server-sent events")
            .about("Committed events from index `from`, then live ones, each to its audience.")
            .query::<super::EventsQuery>("{ from?: uint }")
            .events("each committed event the caller is in the audience of")
            .errors(&[(429, "RATE_LIMITED"), (503, "UNAVAILABLE")]),
        post("/apps/{app}/upgrade", "app_upgrade", AccountHolder, "Upgrade an app instance")
            .about("To a release (`version`, of the app's own package unless `package` names another) or a `bundle`, code only: the state stays and is read by the new code. Whoever the package's manifest names (`upgrades`: the owner, or its publisher's `MANAGE_APPLICATION` holders), with a step-up over `{ bundle, migrate, manifest }`; an upgrade key signs in place of it. A refused or failed upgrade leaves the app on its release and answers `422 UPGRADE_FAILED` with the app log's error in `details` (`MigrationsNotSupported` when `migrate` is named: there are no migrations in this version). `error` is the app log's error value (`null` when it ran), free-form: the app's own.")
            .json::<super::UpgradeBody>("{ bundle?: string, package?: string, version?: string, service?: string, confirm?: #Confirm, migrate?: string }")
            .ok("{ index: uint, status: string, error: any, compute: bool }")
            .step_up()
            .errors(&[(400, "VERSION_REQUIRED"), (400, "BAD_BUNDLE"), (400, "AMBIGUOUS_COMPUTE"), (403, "FORBIDDEN"), (409, "CONFLICT"), (409, "RELEASE_DEADLINE"), (409, "RUNTIME_TOO_OLD"), (422, "UPGRADE_FAILED")]),
        post("/apps/{app}/access", "app_access", Session, "Set who may reach an app")
            .json::<super::AccessBody>("{ access: #Access }")
            .ok("{}"),
        post("/apps/{app}/transfer", "app_transfer", AccountHolder, "Hand an app to another account")
            .about("The owner (for an organization's app, while a member and in the app's team), or an organization admin; an organization's app only to a member. Answers `null`.")
            .json::<super::TransferBody>("{ to: string, confirm?: #Confirm }")
            .ok("null")
            .step_up(),
        post("/apps/{app}/move", "app_move", AccountHolder, "Move an app to another organization")
            .about("Its data where it is; its files, runs and quotas go to the target, `org` or app-team access becomes the target's app team of its package. `bringMembers` adds the source's members who reach it, capped at `member`. `from` is the organization it left, `null` for an app of the caller's own; `added` the accounts brought; `revokedInvites` how many invites to its old team were withdrawn.")
            .json::<super::MoveBody>("{ toOrg: string, bringMembers?: bool, confirm?: #Confirm }")
            .ok("{ appId: string, from: string | null, org: string, access: #Access, created: bool, added: string[], revokedInvites: uint }")
            .step_up()
            .errors(&[(409, "FEATURE_NOT_ACTIVE")]),
        post("/apps/{app}/blobs", "blob_upload", AppAccess, "Upload a blob")
            .about("A plain blob, stored with the request's `Content-Type`, which a component app's download answers with: any media type (printable ASCII, 127 bytes at most; another is `400 BAD_REQUEST`), `application/octet-stream` when the request names none. Send the blob's own type, not `*/*`, which would be stored as its type. Any `sealed` parameter, `true` or `false`, is refused `400 USE_FILES_API`: sealed files go through `/files`.")
            .query::<super::BlobUploadQuery>("{ sealed?: bool }")
            .raw("*/*", "the blob; its `Content-Type` is the type it is stored and served with")
            .ok("{ blobId: string, id: string, size: uint, index: uint }")
            .errors(&[(400, "USE_FILES_API"), (413, "TOO_LARGE"), (429, "QUOTA_EXCEEDED")]),
        get("/apps/{app}/blobs", "blob_list", AppAccess, "List an app's blobs")
            .about("Plain blobs only.")
            .ok("{ blobs: { blobId: string, id: string, size: uint }[] }"),
        get("/apps/{app}/blobs/{id}", "blob_download", AppAccess, "Download a blob")
            .about("`{id}` is base58 or hex; `grant` is a link the app made (`BlobRef::grant`).")
            .query::<super::BlobGrant>("{ grant?: string }")
            .any_bytes("the blob: a component app's under the MIME type it was stored with, otherwise `application/octet-stream`")
            .errors(&[(404, "NOT_FOUND"), (503, "UNAVAILABLE")]),
        delete("/apps/{app}/blobs/{id}", "blob_delete", AppAccess, "Forget a blob")
            .about("The owner or an organization admin.")
            .ok("{ index: uint }")
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        get("/apps/{app}/blobs/{id}/info", "blob_info", AppAccess, "A blob's size")
            .ok("{ blobId: string, id: string, size: uint }")
            .errors(&[(404, "NOT_FOUND"), (409, "NOT_SUPPORTED")]),
        post("/apps/{app}/blobs/{id}/revoke", "blob_revoke", AppAccess, "Invalidate a blob's grants")
            .about("The grants made so far for a published blob; the owner or an organization admin.")
            .ok("{ index: uint }")
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        post("/apps/{app}/files", "files_upload", SignedIn, "Upload a sealed file")
            .about("Up to 16 MiB in one request; the owner and the organization's members.")
            .raw("application/octet-stream", "the file")
            .ok("{ blobId: string, id: string, size: uint, sha256: string, index: uint }")
            .errors(&[(403, "FORBIDDEN"), (413, "TOO_LARGE"), (429, "QUOTA_EXCEEDED")]),
        get("/apps/{app}/files", "files_list", SignedIn, "List an app's sealed files")
            .ok("{ files: { blobId: string, id: string, size: uint, sha256: string }[] }"),
        post("/apps/{app}/files/uploads", "upload_begin", SignedIn, "Begin a file upload")
            .about("A session for a file of `size` bytes, sent in parts of `partBytes`.")
            .json::<super::BeginUpload>("{ size: uint }")
            .ok("{ uploadId: string, partBytes: uint, parts: uint, expiresAt: uint }")
            .errors(&[(413, "TOO_LARGE"), (429, "QUOTA_EXCEEDED")]),
        get("/apps/{app}/files/uploads/{upload}", "upload_status", SignedIn, "Where an upload is")
            .ok("{ uploadId: string, partBytes: uint, parts: uint, expiresAt: uint, size: uint, next: uint }"),
        delete("/apps/{app}/files/uploads/{upload}", "upload_abort", SignedIn, "Abort an upload")
            .ok("{}"),
        put("/apps/{app}/files/uploads/{upload}/parts/{k}", "upload_part", SignedIn, "Send one part")
            .about("Part `k` (from 0), raw bytes; a part out of order (`409 PART_OUT_OF_ORDER`), or one that differs from the part sent before as `k` (`409 PART_CONFLICT`), answers the next one expected in `details.next`.")
            .raw("application/octet-stream", "the part")
            .ok("{ next: uint }")
            .errors(&[(400, "BAD_REQUEST"), (400, "BAD_PART"), (409, "PART_CONFLICT"), (409, "PART_OUT_OF_ORDER"), (503, "FILES_UNAVAILABLE")]),
        post("/apps/{app}/files/uploads/{upload}/complete", "upload_complete", SignedIn, "Commit an upload")
            .about("Charges and commits the file; a retry answers the same file. `409 UPLOAD_INCOMPLETE` names the part still missing.")
            .ok("{ blobId: string, id: string, size: uint, sha256: string, index: uint }")
            .errors(&[(409, "UPLOAD_INCOMPLETE")]),
        get("/apps/{app}/files/{id}", "file_info", SignedIn, "One sealed file")
            .about("Its plaintext size and SHA-256, never its key.")
            .ok("{ blobId: string, id: string, size: uint, sha256: string }")
            .errors(&[(404, "NOT_FOUND")]),
        get("/apps/{app}/xcalls", "app_xcalls", SignedIn, "An app's cross-app calls")
            .about("The owner or an organization admin: how many calls wait in its outbox for delivery (`pending`, a count), and its dead letters, oldest first (`atMs`: the entry time it was dead-lettered at, Unix milliseconds).")
            .ok("{ pending: uint, deadLetters: #DeadLetter[] }")
            .errors(&[(403, "FORBIDDEN"), (503, "UNAVAILABLE")]),
        // ---------------------------------------------------------------- organizations
        post("/orgs", "org_create", Session, "Create an organization")
            .about("`recoveryKey` is the public half of an organization recovery key.")
            .json::<super::OrgBody>("{ name: string, recoveryKey?: string }")
            .ok("{ orgId: string }")
            .errors(&[(429, "QUOTA_EXCEEDED")]),
        get("/orgs/{org}", "org_get", SignedIn, "One organization")
            .about("As the directory records it, with only the teams the caller reaches or manages and the people they see; and its `mode` (from its app teams), the accounts that may approve its releases (`releaseApprovers`), the keys it publishes under (`publishes`), and `newWorkStopped`: the release whose passed deadline stops new work in it (`after`, RFC 3339), `null` when none does. Those four are absent from a node older than they are (1.0.0-rc.1 sends none of them).")
            .ok("#Org")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/orgs/{org}", "org_delete", AccountHolder, "Delete an organization")
            .about("With its teams, invites and apps; the owner. `apps`: the apps deleted with it.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{ apps: string[] }")
            .step_up(),
        post("/orgs/{org}/metadata", "org_metadata", SignedIn, "Set an organization's metadata")
            .about("`metadata` is the caller's own JSON, free-form, kept as given.")
            .json::<super::MetadataBody>("{ metadata: any }")
            .ok("{}"),
        get("/orgs/{org}/usage", "org_usage", SignedIn, "An organization's usage")
            .about("An admin: how many apps the organization develops, what their runs used today (decimal strings) and the cluster's quotas; a member gets `403`.")
            .ok("{ apps: uint, developerFuelToday: u64, developerWritesToday: u64, quotas: #Quotas }")
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        post("/orgs/{org}/upgrade", "org_upgrade", AccountHolder, "Start an upgrade run")
            .about("Upgrade every app of the organization (or of `team`, with `cascade` its subtree) that runs `package` to `version`, with one passkey assertion over the whole run; whoever holds `MANAGE_APPLICATION` there. Each app is upgraded as `POST /apps/{app}/upgrade` would; `GET /orgs/{org}/upgrades/{run}` follows it.")
            .json::<super::OrgUpgradeBody>("{ package: string, version: string, team?: string, cascade?: bool, migrate?: string, confirm?: #Confirm, operatorProof?: #OperatorProof }")
            .ok("#UpgradeRun")
            .step_up()
            .errors(&[(403, "FORBIDDEN")]),
        get("/orgs/{org}/upgrades/{run}", "org_upgrade_status", SignedIn, "An upgrade run's progress")
            .about("A member of the organization.")
            .ok("#UpgradeRun")
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/upgrades/{run}/retry", "org_upgrade_retry", AccountHolder, "Retry an upgrade run's failed apps")
            .about("As a new run under the same terms: a passkey step-up for a member's run, the operator's signature for an operator's run (with the ops token).")
            .opt_value("{ confirm?: #Confirm, operatorProof?: #OperatorProof }")
            .ok("#UpgradeRun")
            .step_up()
            .operator()
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/members/{account}/metadata", "member_metadata", SignedIn, "Set a member's metadata")
            .about("`metadata` is the caller's own JSON, free-form, kept as given.")
            .json::<super::MetadataBody>("{ metadata: any }")
            .ok("{}"),
        get("/orgs/{org}/members", "org_members", AccountHolder, "Members' lockout risk")
            .about("`detail=security` (the only listing: without it, or with another value, `400 BAD_REQUEST`), the owner or an admin: each member's passkeys (`1`, `2` or `\"3+\"`), recovery code, last active UTC day and sessions. `403` below admin, `404` outside the organization.")
            .query::<super::MembersQuery>(r#"{ detail?: "security" }"#)
            .ok(r#"{ orgId: string, members: { account: string, name: string | null, role: #OrgRole, teams: string[], passkeys: uint | "3+", recoveryCode: bool, lastActiveDay: string | null, sessions: uint }[] }"#)
            .errors(&[(400, "BAD_REQUEST"), (403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        post("/orgs/{org}/members", "org_set_member", Session, "Set a member's role")
            .about("`owner` moves ownership. `412` first.")
            .json::<super::MemberBody>("{ account: string, role: #OrgRole, confirm?: #Confirm }")
            .ok("{}")
            .step_up(),
        delete("/orgs/{org}/members/{account}", "org_remove_member", Session, "Remove a member, or leave")
            .about("`412` first; the retry's JSON body is `{ confirm }`.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up(),
        post("/orgs/{org}/members/{account}/teams", "member_set_teams", Session, "Set a member's rows in several teams")
            .about("Each team judged as `POST /orgs/{org}/teams/{team}/members` would judge it, and none set unless every one may be (a row's absent `role` is `member`; absent `capabilities`, a new row takes the defaults and an existing row keeps its own). 1 to 64 distinct teams; one passkey, `412` first.")
            .json::<super::TeamRowsBody>("{ teams: { team: string, role?: #TeamRole, capabilities?: uint }[], confirm?: #Confirm }")
            .ok("{}")
            .step_up()
            .errors(&[(400, "BAD_TEAMS"), (409, "FEATURE_NOT_ACTIVE")]),
        delete("/orgs/{org}/members/{account}/teams", "member_remove_teams", Session, "Remove a member from several teams, or leave them")
            .about("Each judged as `DELETE /orgs/{org}/teams/{team}/members/{account}` would judge it, and none taken unless every one may be. 1 to 64 distinct teams; one passkey, `412` first.")
            .json::<super::TeamIdsBody>("{ teams: string[], confirm?: #Confirm }")
            .ok("{}")
            .step_up()
            .errors(&[(400, "BAD_TEAMS"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/orgs/{org}/teams", "team_create", Session, "Create a team")
            .about("An admin, a lead of `parent`, a member with `CREATE_TEAM` at the top, or a member who holds `CREATE_CHILD_TEAM` under `parent` (a restricted team only).")
            .json::<super::TeamBody>("{ name: string, parent?: string, join: #TeamJoin }")
            .ok("{ teamId: string }"),
        get("/orgs/{org}/teams/{team}", "team_get", SignedIn, "One team")
            .ok("#TeamView")
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/teams/{team}", "team_update", Session, "Update a team")
            .about("Rename, open or restrict, move under another parent, or set its metadata (the caller's own JSON, free-form, kept as given). `parent` absent leaves the team where it is; `parent: null` (an explicit null) moves it to the top; a team id moves it under that team.")
            .json::<super::TeamUpdateBody>("{ name?: string, join?: #TeamJoin, parent?: string | null, metadata?: any }")
            .ok("{}"),
        delete("/orgs/{org}/teams/{team}", "team_delete", Session, "Delete a team")
            .about("With the teams and apps below it.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{ teams: string[], apps: string[] }")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/capabilities", "org_capabilities", Session, "Set capabilities")
            .about("A member's (`account`), in a team (`team`), or a team's default; the capability bits. `412` first.")
            .json::<super::CapabilitiesBody>("{ account?: string, team?: string, capabilities: uint, confirm?: #Confirm }")
            .ok("{}")
            .step_up(),
        post("/orgs/{org}/releases", "org_release_approval", AccountHolder, "Approve a mero-tee release for the organization's apps")
            .about("The owner; `412` first.")
            .json::<super::ReleaseApprovalBody>(APPROVAL)
            .ok("#ApprovedReleases")
            .step_up(),
        post("/orgs/{org}/publishers", "org_publisher", AccountHolder, "Bind a package signing key")
            .about("The key's signature over the binding proves the organization holds it; then its `MANAGE_APPLICATION` holders upgrade the apps of packages that name `publisher`. Answers the key's binding generation now.")
            .json::<super::PublisherBody>("{ signerId: string, publicKey: string, signature: string, confirm?: #Confirm }")
            .ok("{ generation: uint }")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        post("/orgs/{org}/packages/{package}/upgrade", "publisher_upgrade", AccountHolder, "Upgrade every instance of a publisher's package")
            .about("An upgrade run over every app on the cluster that runs the package, by its publisher's `MANAGE_APPLICATION` holders.")
            .json::<super::PublisherUpgradeBody>("{ version: string, migrate?: string, confirm?: #Confirm }")
            .ok("#UpgradeRun")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        get("/orgs/{org}/apps", "org_apps", SignedIn, "The organization's apps")
            .about("As `GET /auth/apps`, for the apps the caller reaches or manages; `404` to a non-member.")
            .query::<super::PageQuery>(PAGE)
            .ok("{ scope: #AppScope, governance: #Governance, approvedReleases: string[] | null, clusterReleases: #ClusterRelease[], apps: #ReportApp[], next: string | null }")
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/outbound", "org_outbound_approval", AccountHolder, "Approve outbound prefixes for the organization's apps")
            .about("The owner; `412` first.")
            .json::<super::OutboundApprovalBody>(OUTBOUND)
            .ok("#OutboundApproved")
            .step_up(),
        post("/orgs/{org}/teams/{team}/members", "team_set_member", Session, "Add or change a team member")
            .json::<super::TeamMemberBody>("{ account: string, lead?: bool, role?: #TeamRole, capabilities?: uint, confirm?: #Confirm }")
            .ok("{}")
            .step_up(),
        delete("/orgs/{org}/teams/{team}/members/{account}", "team_remove_member", Session, "Remove a team member, or leave")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up(),
        post("/orgs/{org}/invites", "invite_create", Session, "Make an invite")
            .about("A token for `role` and `teams`, `uses` times within `ttlSecs`; `allApps` (an admin's) also gives every app added later. `412` first.")
            .json::<super::InviteBody>("{ role: #OrgRole, teams?: string[], ttlSecs?: uint, uses?: uint, allApps?: bool, confirm?: #Confirm }")
            .ok("{ token: string }")
            .step_up()
            .errors(&[(400, "WRONG_CHALLENGE"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/orgs/{org}/name", "org_rename", Session, "Rename an organization")
            .about("The owner or an admin, to a name that fits as a new organization's would; `412` first.")
            .json::<super::RenameOrgBody>("{ name: string, confirm?: #Confirm }")
            .ok("{ orgId: string, name: string }")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/orgs/{org}/all-apps", "org_all_apps", Session, "Give a member every app added later")
            .about("Or stop it (`allApps: false`); apps already there are not touched. The owner or an admin; `412` first.")
            .json::<super::AllAppsBody>("{ account: string, allApps: bool, confirm?: #Confirm }")
            .ok("{}")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "FEATURE_NOT_ACTIVE")]),
        get("/orgs/{org}/invites", "invite_list", Session, "The organization's invites")
            .about("Their keys and terms, never a token: `expiresAt` Unix seconds; `createdBy` and `createdAt` are `null` on an invite from before they were kept.")
            .ok("{ invites: { inviteKey: string, role: #OrgRole, teams: string[], usesLeft: uint, expiresAt: uint, expired: bool, createdBy: string | null, createdAt: uint | null, allApps: bool }[] }")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/orgs/{org}/invites/{key}", "invite_revoke", Session, "Revoke an invite")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        get("/orgs/{org}/app-teams", "app_team_list", SignedIn, "The organization's app teams")
            .about("A member: each app team's package, id, name, `join`, member count, default capabilities, and whether the caller is in it or manages it (a guest: only their own).")
            .ok(r#"{ mode: "legacy" | "standalone" | "shared", appTeams: { package: string, teamId: string, name: string, join: #TeamJoin, memberCount: uint, defaultCapabilities: uint | null, member: bool, manages: bool }[] }"#)
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/app-teams", "app_teams_scope", Session, "Give apps app teams, or open or close one")
            .about("The owner or an admin; `keep` names who stays when a team closes; `childTeams` says whether the app team's members make restricted teams below it (private channels), adding or removing `CREATE_CHILD_TEAM` (absent: as they are). Naming `join` or `childTeams` on any package leaves `shared` unset to mean no other team. `412` first.")
            .json::<super::ScopeAppsBody>("{ apps?: { package: string, name?: string, instances?: string[], join?: #TeamJoin, keep?: string[], childTeams?: bool }[], shared?: bool, confirm?: #Confirm }")
            .ok(r#"{ mode: "legacy" | "standalone" | "shared", teams: { package: string, teamId: string, created: bool, apps: string[], added: uint, join: #TeamJoin | null, dropped: uint }[] }"#)
            .step_up()
            .errors(&[(400, "TOO_MANY_PACKAGES"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/orgs/{org}/recovery", "recovery_set", AccountHolder, "Set the organization's recovery key")
            .about("Set or rotate it (`publicKey`: base64url Ed25519 public key), or remove it (`publicKey: null`; an absent `publicKey` removes it too); the owner; cancels a recovery under way. `412` first.")
            .json::<super::RecoveryKeyBody>("{ publicKey?: string | null, confirm?: #Confirm }")
            .ok("{}")
            .step_up()
            .errors(&[(400, "BAD_RECOVERY_KEY")]),
        post("/orgs/{org}/recovery/start", "recovery_start", AccountHolder, "Start an organization recovery")
            .about("The recovery seed's signature handing the organization to the caller.")
            .json::<super::RecoveryStartBody>("{ nonce: string, signature: string }")
            .ok("{ startedAt: uint, readyAt: uint }")
            .errors(&[(403, "WRONG_RECOVERY_KEY"), (409, "RECOVERY_PENDING")]),
        post("/orgs/{org}/recovery/cancel", "recovery_cancel", AccountHolder, "Cancel an organization recovery")
            .about("The owner; `412` first.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/recovery/finish", "recovery_finish", AccountHolder, "Finish an organization recovery")
            .about("The account that started it, once `readyAt` passed; `412` first.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{ org: string, previousOwners: string[] }")
            .step_up()
            .errors(&[(404, "NOT_FOUND"), (409, "RECOVERY_NOT_READY")]),
        post("/invites/{token}/redeem", "invite_redeem", Session, "Join with an invite")
            .about("No passkey prompt: the node signs the redemption with the token's key. Answers the organization joined.")
            .ok("{ org: string }"),
        // ---------------------------------------------------------------- consent
        post("/consent/requests", "crate::consent_page::create", Session, "Ask the account's owner to approve")
            .about("From an app's frontend: a release approval, an upgrade of its instance, or outbound prefixes, approved on the node's own consent page (`url`), which takes the passkey. 64 KiB at most.")
            .bytes_json::<crate::consent_page::CreateBody>(r#"{ kind: "release", scope: "account" | { org: string }, subject: { version: string, withdraw?: bool }, returnTo: string } | { kind: "upgrade", subject: { app: string, toVersion: string }, returnTo: string } | { kind: "outbound", scope: "account" | { org: string }, subject: { package?: string, prefixes?: string[] }, returnTo: string }"#)
            .ok("{ requestId: string, url: string, code: string, expiresAt: uint }")
            .errors(&[(400, "BAD_REQUEST"), (413, "TOO_LARGE")]),
        get("/consent/{request}/result", "crate::consent_page::result", Session, "What became of a consent request")
            .about("`decidedAt` (Unix seconds) is `null` while it is pending.")
            .ok(r#"{ requestId: string, kind: "release" | "upgrade" | "outbound", status: "pending" | "approved" | "declined", decidedAt: uint | null }"#)
            .errors(&[(404, "NOT_FOUND"), (503, "UNAVAILABLE")]),
        // ---------------------------------------------------------------- ops
        get("/ops/status", "ops_status", Ops, "The node and the cluster")
            .about("Counts, Raft status and governance; never app state, names or credentials. `groups`: the app groups this node hosts; `apps` and `accounts` are counts; `rollback` is this node's rollback counter, `null` on a node without one.")
            .ok("{ node: uint, system: #GroupStatus, governance: #Governance, groups: { app: string, status: #GroupStatus }[], nodes: #NodeRecord[], apps: uint, accounts: uint, rollback: #RollbackStatus | null }"),
        get("/ops/cluster", "ops_cluster", Ops, "The cluster's identity")
            .about("The cluster's name and the identity this replica holds (hex).")
            .ok("{ cluster: string, identity: string }"),
        get("/ops/metrics", "ops_metrics", Ops, "Prometheus metrics")
            .about("Including per-app usage for billing. `--metrics-listen ADDR` also serves it as plain `GET /metrics` there.")
            .text("the Prometheus text format"),
        get("/ops/placement", "ops_placement", Ops, "Where every app's replicas are")
            .about("Ids and counts only, read after a read index (`fresh`: this node caught up with the system leader); `release` is a node's attested release; `clusterVersion` is the logged cluster version (`null` until the system leader logs one).")
            .ok("{ fresh: bool, apps: { app: string, group: uint, replicas: uint[] }[], nodes: { id: uint, zone: string, role: #NodeRole, release: string | null, apps: uint }[], clusterVersion: string | null }"),
        get("/ops/apps", "ops_apps", Ops, "What each app runs")
            .about("Each app's bundle, package, publisher, who holds its upgrades and release approvals, its last upgrade, approved releases, replicas' releases and `lastWriteAt`.")
            .query::<super::PageQuery>(PAGE)
            .ok("{ fresh: bool, governance: #Governance, apps: #ReportApp[], next: string | null }"),
        get("/ops/orgs", "ops_orgs", Ops, "Each organization's approved releases")
            .query::<super::PageQuery>(PAGE)
            .ok("{ fresh: bool, governance: #Governance, orgs: { org: string, approvedReleases: string[] | null }[], next: string | null }"),
        get("/ops/served-origins", "origins::ops_served", Ops, "Custom domains to serve")
            .about("Every served origin a package's manager registered, by the package's label, never its name; `addedAt` is RFC 3339.")
            .ok("{ fresh: bool, origins: { origin: string, host: string, label: string, addedAt: string }[] }"),
        post("/ops/nodes", "ops_register_node", Ops, "Register a node")
            .about("Signed with the operator key over `cluster / __register_node / { id, addr, zone }`. The system leader adds it as a learner and promotes caught-up learners while fewer than 3 voters are left; `role: job` registers a compute node (in `pool`).")
            .json::<super::NodeBody>(r#"{ id: uint, addr: string, zone?: string, operatorProof?: #OperatorProof, role?: "replica" | "job", pool?: string }"#)
            .ok("{}")
            .operator()
            .errors(&[(400, "BAD_ADDRESS"), (400, "BAD_POOL"), (400, "POOL_FOR_JOB_NODES"), (409, "ADDRESS_TAKEN"), (400, "NODE_HOLDS_CLUSTER")]),
        delete("/ops/nodes/{id}", "ops_remove_node", Ops, "Remove a node")
            .about("One no app keeps a replica on; the system leader (elsewhere `421 NOT_LEADER`); signed over `__remove_node / { id }`.")
            .proof_body()
            .ok("{ status: string }")
            .operator()
            .errors(&[(400, "LAST_NODE"), (421, "NOT_LEADER")]),
        get("/ops/nodes/{id}/receipt-key", "account_proof::ops_receipt_key", Ops, "A node's receipt key")
            .ok("{ node: uint, key: string, release: string, attestedAt: uint }")
            .errors(&[(404, "NOT_FOUND")]),
        post("/ops/apps/{app}/replicas", "ops_set_replicas", Ops, "Move an app's replicas")
            .about("Signed over `<app> / __set_replicas / { replicas }` (sorted); at least `min(3, nodes)` registered nodes, each on a release the app's owner approved. The set it already has answers `unchanged: true`. `status` is the app group's Raft status when the move finished (as the leader saw it).")
            .json::<super::ReplicaBody>("{ replicas: uint[], operatorProof?: #OperatorProof }")
            .ok("{ status: { group: uint, leader: uint | null, is_leader: bool, term: uint, last_log: uint | null, applied: uint, voters: uint[], learners: uint[] }, unchanged: bool }")
            .operator()
            .errors(&[(400, "DUPLICATE_REPLICA"), (400, "JOB_NODE"), (400, "TOO_FEW"), (400, "UNKNOWN_NODE"), (409, "MOVE_IN_PROGRESS"), (409, "RUNTIME_TOO_OLD"), (421, "NOT_LEADER")]),
        post("/ops/apps/{app}/restore", "restore::ops_restore", Ops, "Restore an app from its sealed backup")
            .about("Signed over `<app> / __restore_app / { backup, sha256 }`. The receiving node holds the backup (`backup`, else the newest, in `{backup-dir}/{app}/`) and seeds the restore; answers once every replica holds the restored state: the rows restored, the backup (`format` 0, with `merod` and `release` `null`, for one written before manifests) and the app group's Raft status.")
            .json::<super::restore::RestoreBody>("{ backup?: string, operatorProof?: #OperatorProof }")
            .ok("{ app: string, group: uint, rows: uint, backup: { name: string, index: uint, format: uint, merod: string | null, release: string | null }, status: #GroupStatus }")
            .operator()
            .errors(&[(400, "BAD_BACKUP"), (400, "NOT_A_REPLICA_NODE"), (403, "FORBIDDEN"), (404, "BACKUP_NOT_FOUND"), (409, "BACKUP_FROM_NEWER_RELEASE"), (409, "BUNDLE_GONE"), (409, "MOVE_IN_PROGRESS"), (409, "NO_BACKUP_DIR"), (409, "UNKNOWN_BACKUP_FORMAT"), (409, "UPGRADE_PENDING")]),
        post("/ops/backup", "ops_backup", Ops, "Snapshot the apps this node leads")
            .about("Into `--backup-dir`. With `all=true`, every group this node holds, leader or not: what a node of a cluster that lost a majority still has, to found the cluster again from (`merod --bootstrap --restore-from`). `written`: how many snapshots it wrote.")
            .query::<super::BackupQuery>("{ all?: bool }")
            .ok("{ written: uint }"),
        post("/ops/releases", "ops_release", Ops, "Approve a mero-tee release for every app")
            .about("Where the operator holds release approvals.")
            .json::<super::OpsReleaseBody>("{ version: string, approved?: bool, operatorProof?: #OperatorProof }")
            .ok("#ApprovedReleases")
            .operator(),
        post("/ops/releases/{version}/deadline", "ops_release_deadline", Ops, "Set a release deadline")
            .about("After `after` (RFC 3339), apps whose approver has not moved past the release may start no new work.")
            .json::<super::OpsDeadlineBody>("{ after?: string, operatorProof?: #OperatorProof }")
            .ok("#Deadlines")
            .operator(),
        delete("/ops/releases/{version}/deadline", "ops_release_deadline_lift", Ops, "Lift a release deadline")
            .proof_body()
            .ok("#Deadlines")
            .operator(),
        post("/ops/deployers", "ops_deployer_grant", Ops, "Grant a deployer")
            .about("An account or organization that may install where deployments are granted, `until` an RFC 3339 time.")
            .json::<super::DeployerBody>("{ deployer: #Deployer, until?: string, operatorProof?: #OperatorProof, confirm?: #Confirm }")
            .ok("{ key: string, deployer: #Deployer, grant: #DeployerGrant }")
            .operator(),
        delete("/ops/deployers/{key}", "ops_deployer_revoke", Ops, "Revoke a deployer")
            .proof_body()
            .ok("{ key: string }")
            .operator(),
        post("/ops/deployers/{key}/revoke", "ops_deployer_revoke", Ops, "Revoke a deployer (POST)")
            .about("The same as the `DELETE`, for an HTTP client whose `DELETE` carries no body.")
            .proof_body()
            .ok("{ key: string }")
            .operator(),
        post("/ops/deployments/granted", "ops_deployments_granted", Ops, "Require deployer grants")
            .about("From now on only granted deployers install; once.")
            .proof_body()
            .ok("{ governance: #Governance }")
            .operator()
            .errors(&[(409, "CONFLICT")]),
        post("/ops/cluster-owner", "ops_cluster_owner", Ops, "Name the cluster's owner")
            .about("The account or organization that then runs sign-up and deployers with passkeys; once.")
            .json::<super::ClusterOwnerBody>("{ owner: #Deployer, operatorProof?: #OperatorProof }")
            .ok("{ owner: #Deployer }")
            .operator()
            .errors(&[(409, "CONFLICT")]),
        post("/ops/sign-up", "ops_sign_up_set", Ops, "Set the sign-up policy")
            .about("Refused once the cluster's owner runs sign-up.")
            .json::<super::SignUpBody>("{ policy: #SignUp, operatorProof?: #OperatorProof, confirm?: #Confirm }")
            .ok("#SignUpSet")
            .operator(),
        post("/ops/sign-up/invites", "ops_sign_up_invite_create", Ops, "Issue a cluster sign-up invite")
            .json::<super::OpsSignUpInviteBody>("{ token: string, uses?: uint, expiresAt: string, operatorProof?: #OperatorProof }")
            .ok("#SignUpInvite")
            .operator()
            .errors(&[(400, "BAD_REQUEST")]),
        get("/ops/sign-up/invites", "sign_up_invite_list", Ops, "Cluster sign-up invites")
            .ok(r#"{ signUp: { policy: #SignUp, orgInvites: bool, signUpInvites: bool, inviteRequired: bool }, runBy: "owner" | "operator", invites: #SignUpInvite[] }"#)
            .errors(&[(403, "FORBIDDEN")]),
        delete("/ops/sign-up/invites/{key}", "ops_sign_up_invite_revoke", Ops, "Withdraw a cluster sign-up invite")
            .proof_body()
            .ok("#InviteKey")
            .operator(),
        post("/ops/sign-up/invites/{key}/revoke", "ops_sign_up_invite_revoke", Ops, "Withdraw a cluster sign-up invite (POST)")
            .about("For an HTTP client whose `DELETE` carries no body.")
            .proof_body()
            .ok("#InviteKey")
            .operator(),
        post("/ops/outbound", "ops_outbound", Ops, "Approve outbound prefixes")
            .about("Where the operator holds outbound approvals.")
            .json::<super::OpsOutboundBody>("{ package: string, prefixes?: #OutboundSelection, replace?: bool, operatorProof?: #OperatorProof }")
            .ok("#OutboundApproved")
            .operator(),
        post("/ops/apps/{app}/upgrade", "ops_upgrade", Ops, "Upgrade an app as the operator")
            .about("To a release the operator installed, where the operator holds app upgrades. `error` is the app log's error value (`null` when it ran), free-form: the app's own.")
            .json::<super::OpsUpgradeBody>("{ bundle?: string, package?: string, version?: string, service?: string, migrate?: string, operatorProof?: #OperatorProof }")
            .ok("{ index: uint, status: string, error: any, compute: bool }")
            .operator()
            .errors(&[(400, "AMBIGUOUS_COMPUTE"), (400, "BAD_REQUEST"), (409, "NOT_INSTALLED"), (409, "RELEASE_DEADLINE"), (409, "RUNTIME_TOO_OLD"), (422, "UPGRADE_FAILED")]),
        post("/ops/orgs/{org}/upgrade", "ops_org_upgrade", Ops, "An operator's upgrade run")
            .json::<super::OrgUpgradeBody>("{ package: string, version: string, team?: string, cascade?: bool, migrate?: string, confirm?: #Confirm, operatorProof?: #OperatorProof }")
            .ok("#UpgradeRun")
            .operator()
            .errors(&[(404, "NOT_FOUND")]),
        post("/ops/packages", "ops_package_upload", Ops, "Install an uploaded release as the operator")
            .raw("application/octet-stream", "the `.mpk`")
            .ok("{ package: string, version: string, signerId: string, services: { [string]: string } }")
            .errors(&[(400, "BAD_PACKAGE"), (400, "BAD_BUNDLE"), (400, "COMPUTE_ABI_MISMATCH"), (400, "UNSUPPORTED_BUNDLE")]),
        get("/ops/packages", "ops_package_list", Ops, "The releases the operator installed").ok("{ packages: #Package[] }"),
        post("/ops/packages/install", "ops_package_install", Ops, "Install a release from the registry as the operator")
            .json::<super::InstallBody>("{ package: string, version: string }")
            .ok("{ package: string, version: string, signerId: string, services: { [string]: string } }")
            .errors(&[(400, "BAD_BUNDLE"), (400, "COMPUTE_ABI_MISMATCH"), (400, "UNSUPPORTED_BUNDLE")]),
        delete("/ops/packages/{package}", "ops_package_uninstall", Ops, "Uninstall what the operator installed").ok("#Uninstalled"),
        delete("/ops/packages/{package}/{version}", "ops_package_version_uninstall", Ops, "Uninstall one operator-installed version").ok("#Uninstalled"),
        get("/ops/quotas", "ops_quotas_get", Ops, "The quotas")
            .about("The per-account quotas, and the per-payer overrides under `orgs`.")
            .ok("#QuotaSettings"),
        post("/ops/quotas", "ops_quotas_set", Ops, "Change quotas")
            .about("Changes the limits the body names, the others keep their value; signed over `cluster / __set_quotas / { quotas }` with every limit after the change. Without `org`, the cluster's limits (`GET /ops/quotas` names them); `orgs`, which that answer carries, is ignored, so its answer may be sent back changed. With `org`: one payer's `maxConcurrentRuns`, `computeFuel` (decimal string) and `maxFileBytes` (`null` clears one, back to the cluster's), and no other limit. A property that is no limit (of the cluster's, or with `org` of a payer's) is refused `400 UNKNOWN_QUOTA`.")
            .value("#QuotaChanges")
            .ok("{}")
            .operator()
            .errors(&[(400, "BAD_REQUEST"), (400, "UNKNOWN_QUOTA"), (409, "UPGRADE_PENDING")]),
        get("/ops/pools", "ops_pools_get", Ops, "Compute pools")
            .about("The pools by name (absent: none set, one legacy pool), and each pool's capacity: how many runs its job nodes take at once.")
            .ok("{ pools: { [string]: { runsPerNode: uint, maxMemoryMib: uint } }, capacity: { [string]: uint } }"),
        post("/ops/pools", "ops_pools_set", Ops, "Set the compute pools")
            .about("Signed over `cluster / __set_pools / { pools }`; `POOL_IN_USE` when a pool to remove still has job nodes or runs.")
            .json::<super::PoolsBody>("{ pools: { [string]: { runsPerNode: uint, maxMemoryMib: uint } }, operatorProof?: #OperatorProof }")
            .ok("{}")
            .operator()
            .errors(&[(409, "POOL_IN_USE"), (409, "UPGRADE_PENDING")]),
        // ---------------------------------------------------------------- the cluster's owner
        post("/sign-up", "sign_up_set", AccountHolder, "Set the sign-up policy")
            .about("The cluster's owner, once named, with a passkey (`412` first).")
            .json::<super::SignUpBody>("{ policy: #SignUp, operatorProof?: #OperatorProof, confirm?: #Confirm }")
            .ok("#SignUpSet")
            .step_up(),
        post("/sign-up/invites", "sign_up_invite_create", AccountHolder, "Issue a cluster sign-up invite")
            .json::<super::SignUpInviteBody>("{ uses?: uint, ttlSecs?: uint, confirm?: #Confirm }")
            .ok("{ token: string, inviteKey: string, uses: uint, expiresAt: string, expiresAtSecs: uint, createdBy: string, createdAt: string }")
            .step_up()
            .errors(&[(400, "BAD_REQUEST"), (400, "WRONG_CHALLENGE")]),
        get("/sign-up/invites", "sign_up_invite_list", AccountHolder, "Cluster sign-up invites")
            .ok(r#"{ signUp: { policy: #SignUp, orgInvites: bool, signUpInvites: bool, inviteRequired: bool }, runBy: "owner" | "operator", invites: #SignUpInvite[] }"#)
            .errors(&[(403, "FORBIDDEN")]),
        delete("/sign-up/invites/{key}", "sign_up_invite_revoke", AccountHolder, "Withdraw a cluster sign-up invite")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("#InviteKey")
            .step_up(),
        post("/deployers", "deployer_grant", AccountHolder, "Grant a deployer")
            .about("The cluster's owner, with a passkey.")
            .json::<super::DeployerBody>("{ deployer: #Deployer, until?: string, operatorProof?: #OperatorProof, confirm?: #Confirm }")
            .ok("{ key: string, deployer: #Deployer, grant: #DeployerGrant }")
            .step_up(),
        get("/deployers", "deployer_list", AccountHolder, "The deployers")
            .about("The operator, or the cluster's owner: the deployments and deployers governance, the owner (`null` until named), and every grant (`until` RFC 3339 and `untilSecs`, `null` for none; `grantedAt` RFC 3339; `deployer` `null` for a key it cannot read).")
            .ok(r#"{ deployments: "open" | "granted", deployers: "operator" | "owner", owner: #Deployer | null, grants: { key: string, deployer: #Deployer | null, until: string | null, untilSecs: uint | null, grantedAt: string, grantedBy: string }[] }"#)
            .errors(&[(403, "FORBIDDEN")]),
        delete("/deployers/{key}", "deployer_revoke", AccountHolder, "Revoke a deployer")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{ key: string }")
            .step_up(),
        post("/deployments", "deployment_create", AccountHolder, "Create a deployment")
            .about("A named set of releases an owner (an account or organization) offers for others to instantiate. Answers its id, the record as made (no apps yet) and its frontend host (`null` on a node with no apps domain).")
            .json::<super::DeploymentBody>("{ name: string, owner: #Deployer, confirm?: #Confirm }")
            .ok("{ id: string, deployment: #DeploymentRecord, host: string | null }")
            .step_up()
            .errors(&[(403, "FORBIDDEN")]),
        get("/deployments", "deployment_list", SignedIn, "Deployments")
            .about("The deployments the caller manages.")
            .ok("{ deployments: #Deployment[] }"),
        get("/deployments/{id}", "deployment_get", SignedIn, "One deployment")
            .about("As anyone may read it: its name, its owner's kind and name (`null` when unknown), and the releases it offers; never its managers or customers.")
            .ok(r#"{ id: string, host: string | null, name: string, owner: { kind: "account" | "org", name: string | null }, apps: { app: string, package: string, version: string }[] }"#)
            .errors(&[(404, "NOT_FOUND")]),
        delete("/deployments/{id}", "deployment_delete", AccountHolder, "Delete a deployment")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{ deployment: string }")
            .step_up(),
        post("/deployments/{id}/apps", "deployment_offer", AccountHolder, "Offer a release in a deployment")
            .json::<super::OfferBody>("{ app: string, package: string, version: string, confirm?: #Confirm }")
            .ok("{ deployment: string, app: string }")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        delete("/deployments/{id}/apps/{app}", "deployment_withdraw", AccountHolder, "Withdraw an offer")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{ deployment: string, app: string }")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        get("/deployments/{id}/orgs", "deployment_orgs", SignedIn, "The organizations a deployment serves")
            .about("For its managers: each organization with instances from it, its name (`null` when unknown), member counts (`activeMembers30d`: active in the last 30 days), its instances and the releases they run, and when its first was created (RFC 3339); instances outside an organization are only counted (`outsideOrgs`). Never who the members are.")
            .ok("{ deployment: string, orgs: { orgId: string, name: string | null, memberCount: uint, activeMembers30d: uint, instances: uint, apps: { app: string, version: string }[], firstCreatedAt: string }[], outsideOrgs: uint }"),
    ]
}

/// The tag of a path: the section of the reference it is listed in.
fn tag(path: &str) -> &'static str {
    let first = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();
    match first {
        "health" | "attestation" | "releases" | "publishers" => "Public",
        "auth" => "Accounts",
        "bundles" | "packages" | "package-origins" => "Packages",
        "apps" => "Apps",
        "orgs" | "invites" => "Organizations",
        "consent" => "Consent",
        "ops" => "Ops",
        _ => "Sign-up and deployments",
    }
}

/// `x-calimero-group`: the tag as an identifier, for an SDK's namespace of the operation.
fn group(tag: &str) -> &'static str {
    match tag {
        "Public" => "public",
        "Accounts" => "auth",
        "Packages" => "packages",
        "Apps" => "apps",
        "Organizations" => "orgs",
        "Consent" => "consent",
        "Ops" => "ops",
        _ => "deployments",
    }
}

const TAGS: &[(&str, &str)] = &[
    (
        "Public",
        "In the clear (`/health`, `/attestation`) or sealed without a credential.",
    ),
    (
        "Accounts",
        "Sign-up, sign-in, passkeys, sessions, API keys and the account's own approvals.",
    ),
    (
        "Packages",
        "Signed releases (`.mpk`), raw bundles and frontend origins.",
    ),
    (
        "Apps",
        "App instances (contexts): calls, views, events, upgrades, blobs and sealed files.",
    ),
    (
        "Organizations",
        "Organizations, members, teams, invites, approvals and upgrade runs.",
    ),
    (
        "Consent",
        "Approvals an app's frontend asks for on the node's own consent page.",
    ),
    (
        "Sign-up and deployments",
        "The cluster's owner: the sign-up policy, deployers and deployments.",
    ),
    (
        "Ops",
        "The operator's routes: every one needs the ops token; decisions are signed with the operator key.",
    ),
];

/// The paths a client may call without the sealed channel.
const IN_THE_CLEAR: &[&str] = &["/health", "/attestation"];

fn operation_id(op: &Op, ops: &[Op]) -> String {
    let base = |o: &Op| {
        let last = o.handler.rsplit("::").next().unwrap_or(o.handler);
        // `restore::ops_restore` is `ops_restore`, not `restore_ops_restore`.
        let name = if last.starts_with("ops_") {
            last.to_owned()
        } else {
            o.handler.trim_start_matches("crate::").replace("::", "_")
        };
        if o.path.starts_with("/ops/") && !name.starts_with("ops_") {
            format!("ops_{name}")
        } else {
            name
        }
    };
    let id = base(op);
    if ops.iter().filter(|o| base(o) == id).count() > 1 {
        format!("{id}_{}", op.method)
    } else {
        id
    }
}

fn shapes() -> BTreeMap<&'static str, Shape> {
    components()
        .into_iter()
        .map(|c| (c.name, parse(c.shape)))
        .collect()
}

/// Whether `ty` refuses a body with a property its shape does not name.
fn closed(input: &Input, shapes: &BTreeMap<&'static str, Shape>, query: bool) -> bool {
    let Some(ty) = input.ty else {
        return false;
    };
    let refs = |name: &str| shapes[name].clone();
    let mut sample = parse(input.shape).sample(&refs);
    let Some(obj) = sample.as_object_mut() else {
        return false;
    };
    let _prev = obj.insert("zzNotAField".to_owned(), json!("x"));
    (ty.parse)(&sample, query).is_err()
}

/// One status's error answer: the shared `Error`, the codes it may carry listed in
/// `x-calimero-codes` (and described), not narrowed in a schema of its own, which every
/// generator would turn into a type per operation; `x-calimero-details` names the schema of
/// `details` for each of those codes that carries one.
fn error_response(status: u16, codes: &BTreeSet<&'static str>) -> Value {
    let details = details_of(codes.iter().copied());
    let described: Vec<String> = codes
        .iter()
        .map(|code| {
            COMMON_CODES
                .iter()
                .find(|(s, c, _)| *s == status && c == code)
                .map_or_else(
                    || format!("`{code}`"),
                    |(_, _, why)| format!("`{code}`: {why}"),
                )
        })
        .collect();
    let mut description = described.join("; ");
    let named: Vec<String> = details
        .iter()
        .filter_map(|(code, schema)| {
            let name = schema["$ref"].as_str()?.rsplit('/').next()?;
            Some(format!("`{code}`: a `{name}`"))
        })
        .collect();
    if !named.is_empty() {
        description.push_str(&format!(". `details` of {}.", named.join(", ")));
    }
    let mut out = json!({
        "description": description,
        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } },
        "x-calimero-codes": codes,
    });
    if !details.is_empty() {
        out["x-calimero-details"] = Value::Object(details);
    }
    out
}

fn input_schema(input: &Input, shapes: &BTreeMap<&'static str, Shape>) -> Value {
    let mut schema = parse(input.shape).schema();
    // A named body is closed, or not, where it is defined (`Component::closed_body`).
    if schema.get("$ref").is_none() && closed(input, shapes, false) {
        match schema.get_mut("oneOf").and_then(Value::as_array_mut) {
            // Each alternative is closed, not the union.
            Some(alts) => {
                for alt in alts {
                    alt["additionalProperties"] = json!(false);
                }
            }
            None => schema["additionalProperties"] = json!(false),
        }
    }
    schema
}

fn operation(op: &Op, ops: &[Op], shapes: &BTreeMap<&'static str, Shape>) -> Value {
    let mut description = String::from(op.description);
    if !description.is_empty() {
        description.push_str("\n\n");
    }
    description.push_str("**Who:** ");
    description.push_str(op.who.text());
    description.push('.');
    if op.step_up {
        description.push_str(" **Step-up:** ");
        if !op.step_up_when.is_empty() {
            description.push_str("when ");
            description.push_str(op.step_up_when);
            description.push_str(", ");
        }
        description.push_str(
            "answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.",
        );
    }
    if op.operator {
        description.push_str(
            " **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.",
        );
    }
    let mut params: Vec<Value> = op
        .path
        .split('/')
        .filter_map(|seg| seg.strip_prefix('{')?.strip_suffix('}'))
        .map(|name| json!({ "name": name, "in": "path", "required": true, "schema": { "type": "string" } }))
        .collect();
    if let Some(q) = &op.query {
        for f in parse(q.shape).fields() {
            params.push(json!({
                "name": f.name, "in": "query", "required": !f.optional, "schema": f.shape.schema(),
            }));
        }
    }
    let mut out = json!({
        "operationId": operation_id(op, ops),
        "tags": [tag(op.path)],
        "x-calimero-group": group(tag(op.path)),
        "summary": op.summary,
        "description": description,
        "security": op.who.security(),
        "x-calimero-who": op.who.text(),
        "x-calimero-sealed": !IN_THE_CLEAR.contains(&op.path),
    });
    if op.step_up {
        out["x-calimero-step-up"] = json!(true);
        out["x-calimero-user-verification"] = json!(op.user_verification);
        if !op.step_up_when.is_empty() {
            out["x-calimero-step-up-when"] = json!(op.step_up_when);
        }
    }
    if op.operator {
        out["x-calimero-operator"] = json!(true);
    }
    match op.retry {
        Retry::Unsaid => {}
        Retry::Safe => out["x-calimero-retry-safe"] = json!(true),
        Retry::SafeWith(field) => {
            out["x-calimero-retry-safe"] = json!(true);
            out["x-calimero-retry-requires"] = json!(field);
        }
    }
    if !params.is_empty() {
        out["parameters"] = Value::Array(params);
    }
    let body = match &op.body {
        Body::None => None,
        Body::Json(i) | Body::BytesJson(i) => {
            Some((true, "application/json", input_schema(i, shapes), ""))
        }
        Body::OptionalJson(i) | Body::RawJson(i) => Some((
            false,
            "application/json",
            input_schema(i, shapes),
            "May be absent.",
        )),
        Body::Raw(media, what) => Some((
            true,
            *media,
            if *media == "*/*" {
                json!({ "type": "string", "format": "binary" })
            } else {
                json!({ "type": "string", "format": "binary", "contentMediaType": media })
            },
            *what,
        )),
    };
    if let Some((required, media, schema, what)) = body {
        let mut content = Map::new();
        // Any media type: the generic one first, which a generated client sends by default
        // and the node stores when the request names none, then `*/*` for the others.
        if media == "*/*" {
            let _prev = content.insert(
                "application/octet-stream".to_owned(),
                json!({ "schema": schema.clone() }),
            );
        }
        let _prev = content.insert(media.to_owned(), json!({ "schema": schema }));
        out["requestBody"] = json!({
            "required": required,
            "content": content,
        });
        if !what.is_empty() {
            out["requestBody"]["description"] = json!(what);
        }
    }
    let ok = match op.reply {
        Reply::Json(shape) => json!({
            "description": "OK",
            "content": { "application/json": { "schema": parse(shape).schema() } },
        }),
        Reply::Events(what) => json!({
            "description": format!("Server-sent events: {what}."),
            "content": { "text/event-stream": { "schema": { "type": "string" } } },
        }),
        Reply::Text(what) => json!({
            "description": format!("OK: {what}."),
            "content": { "text/plain": { "schema": { "type": "string" } } },
        }),
        Reply::Bytes(media, what) => {
            let mut schema = json!({ "type": "string", "format": "binary" });
            if media != "*/*" {
                schema["contentMediaType"] = json!(media);
            }
            json!({
                "description": format!("OK: {what}."),
                "content": { media: { "schema": schema } },
            })
        }
    };
    let mut responses = Map::new();
    let _prev = responses.insert("200".to_owned(), ok);
    for (status, codes) in op.codes() {
        let _prev = responses.insert(status.to_string(), error_response(status, &codes));
    }
    out["responses"] = Value::Object(responses);
    out
}

/// The OpenAPI 3.1 document of the node API.
pub(super) fn document() -> Value {
    let ops = operations();
    let shapes = shapes();
    let mut paths: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
    for op in &ops {
        let _prev = paths
            .entry(op.path)
            .or_default()
            .insert(op.method.to_owned(), operation(op, &ops, &shapes));
    }
    let schemas: Map<String, Value> = components()
        .into_iter()
        .map(|c| {
            let mut schema = parse(c.shape).schema();
            schema["description"] = json!(c.description);
            if c.closed_body {
                schema["additionalProperties"] = json!(false);
            }
            (c.name.to_owned(), schema)
        })
        .collect();
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Calimero node API",
            "version": "1",
            "description": DESCRIPTION,
            "license": { "name": "MIT OR Apache-2.0", "identifier": "MIT OR Apache-2.0" },
        },
        "servers": [{
            "url": "https://{node}",
            "description": "A node's --listen address, reached through the sealed channel.",
            "variables": { "node": { "default": "localhost:2428" } },
        }],
        "tags": TAGS.iter().map(|(name, description)| json!({ "name": name, "description": description })).collect::<Vec<_>>(),
        "x-calimero-error-details": details_of(ERROR_DETAILS.iter().map(|(code, _)| *code)),
        "x-calimero-sealed-refusal": { "$ref": "#/components/schemas/SealedRefusal" },
        "paths": paths,
        "components": {
            "schemas": schemas,
            "securitySchemes": {
                "session": {
                    "type": "http", "scheme": "bearer", "bearerFormat": "v2.<claims>.<signature>",
                    "description": "A passkey session's token (15 minutes, by the issuing node), from a passkey ceremony or `/auth/refresh`.",
                },
                "apiKey": {
                    "type": "http", "scheme": "bearer", "bearerFormat": "cal_...",
                    "description": "An API key from `POST /auth/apikeys`, scoped to apps, methods and read-only as made.",
                },
                "opsToken": {
                    "type": "http", "scheme": "bearer",
                    "description": "The node's ops token (`--ops-token` / `--ops-token-sha256`).",
                },
            },
        },
    })
}

/// The field names `T`'s `Deserialize` asks for, when it is a plain struct: what
/// `deserialize_struct` is called with, before any value is read.
fn fields_of<T: DeserializeOwned>() -> Option<&'static [&'static str]> {
    #[derive(Debug)]
    struct Stop;
    impl fmt::Display for Stop {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("stop")
        }
    }
    impl std::error::Error for Stop {}
    impl de::Error for Stop {
        fn custom<M: fmt::Display>(_: M) -> Self {
            Self
        }
    }
    struct Tracer<'a>(&'a mut Option<&'static [&'static str]>);
    impl<'de> Deserializer<'de> for Tracer<'_> {
        type Error = Stop;

        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Stop> {
            Err(Stop)
        }

        fn deserialize_struct<V: Visitor<'de>>(
            self,
            _name: &'static str,
            fields: &'static [&'static str],
            _visitor: V,
        ) -> Result<V::Value, Stop> {
            *self.0 = Some(fields);
            Err(Stop)
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
            byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map enum
            identifier ignored_any
        }
    }
    let mut out = None;
    let _stopped = T::deserialize(Tracer(&mut out));
    out
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use serde_json::{json, Value};

    use super::shape::{parse, Shape};
    use super::trace::{self, Step};
    use super::{
        components, document, fields_of, operations, shapes, Body, Input, Op, ERROR_DETAILS,
    };
    use crate::error::ApiError;

    /// The area modules `router` merges, each with its own route table and the
    /// handlers it names without a prefix.
    const AREAS: &[(&str, &str)] = super::super::AREA_SOURCES;

    /// The sources the router's handlers live in, by the prefix `router` names them with.
    const SOURCES: &[(&str, &str)] = &[
        ("", include_str!("../api.rs")),
        ("origins::", include_str!("origins.rs")),
        ("account_proof::", include_str!("account_proof.rs")),
        ("crate::consent_page::", include_str!("../consent_page.rs")),
        ("restore::", include_str!("restore.rs")),
    ];

    /// `(method, path, handler)` of every `.route(..)` in `router` and in the
    /// area routers it merges.
    fn routes() -> Vec<(String, String, String)> {
        let src = SOURCES[0].1;
        let start = src.find("pub fn router(").expect("router");
        let end = start + src[start..].find(".route_layer(").expect("route_layer");
        let mut out = routes_in(&src[start..end]);
        for (area, area_src) in AREAS {
            assert!(
                src[start..end].contains(&format!(".merge({area}::routes())")),
                "`router` does not merge {area}::routes()"
            );
            let at = area_src.find("fn routes()").expect("routes");
            out.extend(routes_in(&area_src[at..]));
        }
        let merged = src[start..end].matches(".merge(").count();
        assert_eq!(
            merged,
            AREAS.len(),
            "`router` merges an area this test does not read: add it to AREAS"
        );
        out
    }

    /// `(method, path, handler)` of every `.route(..)` in `table`.
    fn routes_in(table: &str) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        let mut rest = table;
        while let Some(i) = rest.find(".route(") {
            rest = &rest[i + ".route(".len()..];
            let q1 = rest.find('"').expect("path");
            let q2 = q1 + 1 + rest[q1 + 1..].find('"').expect("path end");
            let path = &rest[q1 + 1..q2];
            // The method router: up to the `)` that closes `.route(`.
            let mut depth = 1;
            let mut j = q2 + 1;
            for (k, c) in rest[q2 + 1..].char_indices() {
                depth += i32::from(c == '(') - i32::from(c == ')');
                if depth == 0 {
                    j = q2 + 1 + k;
                    break;
                }
            }
            let chain: String = rest[q2 + 1..j].split_whitespace().collect();
            for method in ["get", "post", "put", "delete"] {
                let mut at = 0;
                while let Some(k) = chain[at..].find(&format!("{method}(")) {
                    let k = at + k;
                    at = k + method.len() + 1;
                    let prev = chain[..k].chars().last();
                    if prev.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':') {
                        continue;
                    }
                    let end = chain[at..].find(')').expect("handler end");
                    out.push((
                        method.to_owned(),
                        path.to_owned(),
                        chain[at..at + end].to_owned(),
                    ));
                }
            }
        }
        out
    }

    /// The handler's parameter list and body, from its source.
    fn handler(name: &str) -> (String, String) {
        let (prefix, src) = SOURCES
            .iter()
            .filter(|(p, _)| name.starts_with(p))
            .max_by_key(|(p, _)| p.len())
            .expect("a source");
        let fname = &name[prefix.len()..];
        let needle = format!("async fn {fname}(");
        // A handler `router` names without a prefix is in `api.rs` or an area module.
        let src = if prefix.is_empty() && !src.contains(&needle) {
            AREAS
                .iter()
                .map(|(_, s)| *s)
                .find(|s| s.contains(&needle))
                .unwrap_or(src)
        } else {
            src
        };
        let at = src.find(&needle).unwrap_or_else(|| panic!("no {needle}"));
        let open = at + needle.len();
        let mut depth = 1;
        let mut close = open;
        for (k, c) in src[open..].char_indices() {
            depth += i32::from(c == '(') - i32::from(c == ')');
            if depth == 0 {
                close = open + k;
                break;
            }
        }
        let body_open = close + src[close..].find('{').expect("body");
        let mut depth = 0;
        let mut body_close = body_open;
        for (k, c) in src[body_open..].char_indices() {
            depth += i32::from(c == '{') - i32::from(c == '}');
            if depth == 0 {
                body_close = body_open + k;
                break;
            }
        }
        (
            src[open..close]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            src[body_open..=body_close]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    /// The type inside `wrapper<..>` in `sig`, and whether it sits in an `Option<..>`.
    fn extracted(sig: &str, wrapper: &str) -> Option<(String, bool)> {
        let open = format!("{wrapper}<");
        let at = sig.find(&open)?;
        let start = at + open.len();
        let mut depth = 1;
        for (k, c) in sig[start..].char_indices() {
            depth += i32::from(c == '<') - i32::from(c == '>');
            if depth == 0 {
                let optional = sig[..at].ends_with("Option<");
                return Some((sig[start..start + k].to_owned(), optional));
            }
        }
        None
    }

    fn last_segment(ty: &str) -> &str {
        ty.split('<')
            .next()
            .unwrap_or(ty)
            .rsplit("::")
            .next()
            .unwrap_or(ty)
    }

    fn input_matches(input: &Input, ty: &str) -> bool {
        match input.ty {
            Some(t) => last_segment(t.name) == last_segment(ty),
            None => matches!(last_segment(ty), "Value" | "Map"),
        }
    }

    /// `shape` as it stands for `v`: a `#Name` resolved, and of a union the alternative
    /// `v` is a value of (the first, when none is).
    fn resolved(shape: &Shape, v: Option<&Value>, refs: &dyn Fn(&str) -> Shape) -> Shape {
        match shape {
            Shape::Ref(name) => resolved(&refs(name), v, refs),
            Shape::OneOf(alts) => {
                let some = alts.iter().filter(|a| **a != Shape::Null);
                let fits = some
                    .clone()
                    .find(|a| v.is_some_and(|v| a.admits(v, refs)))
                    .or_else(|| some.clone().next())
                    .unwrap_or(&Shape::Null);
                resolved(fits, v, refs)
            }
            other => other.clone(),
        }
    }

    /// The shape of the value at `path` in `sample`, a value of `shape`; `None` where the
    /// shape says nothing of it (`any`).
    fn shape_at(
        shape: &Shape,
        sample: &Value,
        path: &[Step],
        refs: &dyn Fn(&str) -> Shape,
    ) -> Option<Shape> {
        let mut shape = resolved(shape, Some(sample), refs);
        let mut value = Some(sample);
        for step in path {
            let inner = match (&shape, step) {
                (Shape::Object(fields, rest), Step::Key(k)) => fields
                    .iter()
                    .find(|f| f.name == *k)
                    .map(|f| f.shape.clone())
                    .or_else(|| rest.as_deref().cloned())?,
                (Shape::List(item), Step::Item(_)) => (**item).clone(),
                _ => return None,
            };
            value = value.and_then(|v| match step {
                Step::Key(k) => v.get(k),
                Step::Item(i) => v.get(i),
            });
            shape = resolved(&inner, value, refs);
        }
        Some(shape)
    }

    /// The value at `path` in `v`.
    fn value_at_mut<'v>(v: &'v mut Value, path: &[Step]) -> Option<&'v mut Value> {
        path.iter().try_fold(v, |v, step| match step {
            Step::Key(k) => v.get_mut(k),
            Step::Item(i) => v.get_mut(i),
        })
    }

    /// Below the top level, each struct the type reads from `sample` (a value of `shape`)
    /// against the object the shape has there: the same field names. An object that also
    /// takes other properties (`[string]: ..`) names those clients rely on, so a subset.
    /// With `accepts` (whether the type reads a value), a field is also optional there
    /// exactly when the type reads the sample without it. `checked` counts the structs.
    fn nested_mismatches(
        at: &str,
        shape: &Shape,
        sample: &Value,
        reads: Result<Vec<trace::Read>, String>,
        refs: &dyn Fn(&str) -> Shape,
        accepts: Option<&dyn Fn(&Value) -> bool>,
        checked: &mut usize,
    ) -> Vec<String> {
        let reads = match reads {
            Ok(reads) => reads,
            Err(e) => return vec![format!("{at}: the type does not read its sample: {e}")],
        };
        let mut wrong = Vec::new();
        for read in reads.iter().filter(|r| !r.path.is_empty()) {
            *checked += 1;
            let actual: BTreeSet<&str> = read.fields.iter().copied().collect();
            let where_ = trace::show(&read.path);
            match shape_at(shape, sample, &read.path, refs) {
                Some(Shape::Object(fields, rest)) => {
                    let named: BTreeSet<&str> = fields.iter().map(|f| f.name.as_str()).collect();
                    let fits = if rest.is_some() {
                        named.is_subset(&actual)
                    } else {
                        named == actual
                    };
                    if !fits {
                        wrong.push(format!(
                            "{at}: at `{where_}` the type reads {actual:?}, the shape names {named:?}"
                        ));
                        continue;
                    }
                    let Some(accepts) = accepts else { continue };
                    for f in &fields {
                        let mut without = sample.clone();
                        let _gone = value_at_mut(&mut without, &read.path)
                            .and_then(Value::as_object_mut)
                            .and_then(|o| o.remove(&f.name));
                        let accepted = accepts(&without);
                        if accepted != f.optional {
                            wrong.push(format!(
                                "{at}: at `{where_}`, `{}` is {} in the shape but {} by the type",
                                f.name,
                                if f.optional { "optional" } else { "required" },
                                if accepted { "optional" } else { "required" },
                            ));
                        }
                    }
                }
                Some(Shape::Any) | None => {}
                Some(other) => wrong.push(format!(
                    "{at}: at `{where_}` the type reads a struct {actual:?}, the shape has {other:?}"
                )),
            }
        }
        wrong
    }

    #[test]
    fn the_registry_covers_the_router() {
        let routed: BTreeSet<(String, String, String)> = routes()
            .into_iter()
            .filter(|(_, path, _)| !path.starts_with("/internal/"))
            .collect();
        assert!(
            routed.len() > 100,
            "the router parse found {} routes",
            routed.len()
        );
        let documented: Vec<(String, String, String)> = operations()
            .iter()
            .map(|o| (o.method.to_owned(), o.path.to_owned(), o.handler.to_owned()))
            .collect();
        let unique: BTreeSet<_> = documented.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            documented.len(),
            "a route is documented twice"
        );
        let missing: Vec<_> = routed.difference(&unique).collect();
        let stale: Vec<_> = unique.difference(&routed).collect();
        assert!(
            missing.is_empty() && stale.is_empty(),
            "the registry in api/openapi.rs and `router` differ:\n  not documented: {missing:?}\n  not routed: {stale:?}"
        );
    }

    #[test]
    fn each_entry_names_what_its_handler_extracts() {
        let mut wrong = Vec::new();
        for op in operations() {
            let (sig, _) = handler(op.handler);
            let json = extracted(&sig, "Json");
            let raw = sig.contains(": Body") || sig.contains("Bytes");
            let ok = match (&op.body, &json) {
                (Body::None, None) => !raw,
                (Body::Json(i), Some((ty, false))) | (Body::OptionalJson(i), Some((ty, true))) => {
                    input_matches(i, ty)
                }
                (Body::Raw(..) | Body::RawJson(_) | Body::BytesJson(_), None) => raw,
                _ => false,
            };
            let query_ok = match (&op.query, extracted(&sig, "Query")) {
                (None, None) => true,
                (Some(i), Some((ty, false))) => input_matches(i, &ty),
                _ => false,
            };
            if !ok || !query_ok {
                wrong.push(format!("{} {} ({}): {sig}", op.method, op.path, op.handler));
            }
        }
        assert!(
            wrong.is_empty(),
            "entries whose body or query is not their handler's:\n{}",
            wrong.join("\n")
        );
    }

    /// Every input whose type is known: the shape's fields are the type's, a sample of
    /// the shape deserializes into it, and a field is optional exactly when the type
    /// accepts the input without it.
    #[test]
    fn request_shapes_match_their_types() {
        let shapes = shapes();
        let refs = |name: &str| {
            shapes
                .get(name)
                .cloned()
                .unwrap_or_else(|| panic!("no component {name}"))
        };
        let mut wrong = Vec::new();
        let mut nested = 0;
        let comps = components();
        let ops = operations();
        let inputs = ops.iter().flat_map(|op: &Op| {
            let body = match &op.body {
                Body::Json(i) | Body::OptionalJson(i) | Body::RawJson(i) | Body::BytesJson(i) => {
                    Some((i, false))
                }
                Body::None | Body::Raw(..) => None,
            };
            body.into_iter()
                .chain(op.query.iter().map(|q| (q, true)))
                .map(move |(i, query)| (op, i, query))
        });
        for (op, input, query) in inputs {
            let Some(ty) = input.ty else { continue };
            let at = format!("{} {} ({})", op.method, op.path, ty.name);
            // A named body: its component's shape, closed where its type is.
            let shape = match parse(input.shape) {
                Shape::Ref(name) => {
                    let c = comps.iter().find(|c| c.name == name).expect("a component");
                    if c.closed_body != super::closed(input, &shapes, query) {
                        wrong.push(format!(
                            "{at}: #{name} is {} but the type {}",
                            if c.closed_body { "closed" } else { "open" },
                            if c.closed_body {
                                "takes unknown fields"
                            } else {
                                "refuses them"
                            },
                        ));
                    }
                    refs(&name)
                }
                other => other,
            };
            let named: BTreeSet<&str> = shape.fields().iter().map(|f| f.name.as_str()).collect();
            if let Some(fields) = (ty.fields)() {
                let actual: BTreeSet<&str> = fields.iter().copied().collect();
                if actual != named {
                    wrong.push(format!(
                        "{at}: the type reads {actual:?}, the shape names {named:?}"
                    ));
                    continue;
                }
            }
            let sample = shape.sample(&refs);
            if let Err(e) = (ty.parse)(&sample, query) {
                wrong.push(format!(
                    "{at}: a sample of the shape does not deserialize: {e}\n    {sample}"
                ));
                continue;
            }
            // Each alternative of a union, not only the first.
            if let Shape::OneOf(alts) = &shape {
                for alt in alts.iter().filter(|a| **a != Shape::Null) {
                    let sample = alt.sample(&refs);
                    if let Err(e) = (ty.parse)(&sample, query) {
                        wrong.push(format!(
                            "{at}: an alternative's sample does not deserialize: {e}\n    {sample}"
                        ));
                    }
                }
            }
            // The objects inside it, at any depth, each alternative of a union too. A query
            // is flat.
            if !query {
                let alts = match &shape {
                    Shape::OneOf(alts) => alts.clone(),
                    other => vec![other.clone()],
                };
                for alt in alts.iter().filter(|a| **a != Shape::Null) {
                    let sample = alt.sample(&refs);
                    let accepts = |v: &Value| (ty.parse)(v, false).is_ok();
                    wrong.extend(nested_mismatches(
                        &at,
                        alt,
                        &sample,
                        (ty.structs)(&sample),
                        &refs,
                        Some(&accepts),
                        &mut nested,
                    ));
                }
            }
            for f in shape.fields() {
                let mut without = sample.clone();
                let _gone = without.as_object_mut().and_then(|o| o.remove(&f.name));
                let accepted = (ty.parse)(&without, query).is_ok();
                if accepted != f.optional {
                    wrong.push(format!(
                        "{at}: `{}` is {} in the shape but {} by the type",
                        f.name,
                        if f.optional { "optional" } else { "required" },
                        if accepted { "optional" } else { "required" },
                    ));
                }
            }
        }
        assert!(
            nested > 0,
            "no request type reads a struct below its top level"
        );
        assert!(
            wrong.is_empty(),
            "request shapes that differ from their types:\n{}",
            wrong.join("\n")
        );
    }

    /// Every component the node answers by serializing a Rust type: the shape's fields are
    /// the type's, a sample of the shape read into the type writes back as a value of the
    /// shape (each alternative of a union too), and a field is optional in the shape exactly
    /// when the type leaves it out once it is absent.
    #[test]
    fn response_shapes_match_their_types() {
        let shapes = shapes();
        let refs = |name: &str| {
            shapes
                .get(name)
                .cloned()
                .unwrap_or_else(|| panic!("no component {name}"))
        };
        let mut wrong = Vec::new();
        let mut checked = 0;
        let mut nested = 0;
        for c in components() {
            let Some(ty) = c.ty else { continue };
            checked += 1;
            let at = format!("#{} ({})", c.name, ty.name);
            let shape = parse(c.shape);
            if let Some(fields) = (ty.fields)() {
                let actual: BTreeSet<&str> = fields.iter().copied().collect();
                let named: BTreeSet<&str> =
                    shape.fields().iter().map(|f| f.name.as_str()).collect();
                if actual != named {
                    wrong.push(format!(
                        "{at}: the type has {actual:?}, the shape names {named:?}"
                    ));
                    continue;
                }
            }
            let alts = match &shape {
                Shape::OneOf(alts) => alts.clone(),
                other => vec![other.clone()],
            };
            for alt in alts.iter().filter(|a| **a != Shape::Null) {
                let sample = alt.sample(&refs);
                wrong.extend(nested_mismatches(
                    &at,
                    alt,
                    &sample,
                    (ty.structs)(&sample),
                    &refs,
                    None,
                    &mut nested,
                ));
                match (ty.round_trip)(&sample) {
                    Ok(out) if shape.admits(&out, &refs) => {}
                    Ok(out) => wrong.push(format!(
                        "{at}: the type writes a value the shape does not admit:\n    {out}"
                    )),
                    Err(e) => wrong.push(format!(
                        "{at}: a sample of the shape does not deserialize: {e}\n    {sample}"
                    )),
                }
            }
            let sample = shape.sample(&refs);
            if let Ok(Value::Object(out)) = (ty.round_trip)(&sample) {
                for f in shape.fields().iter().filter(|f| !out.contains_key(&f.name)) {
                    wrong.push(format!(
                        "{at}: `{}` is left out of the written sample: give it a sample the type writes",
                        f.name
                    ));
                }
            }
            for f in shape.fields() {
                let mut without = sample.clone();
                let _gone = without.as_object_mut().and_then(|o| o.remove(&f.name));
                let left_out = match (ty.round_trip)(&without) {
                    Ok(out) => {
                        if !shape.admits(&out, &refs) {
                            wrong.push(format!(
                                "{at}: without `{}`, the type writes a value the shape does not admit:\n    {out}",
                                f.name
                            ));
                        }
                        out.get(&f.name).is_none()
                    }
                    Err(_) => false,
                };
                if left_out != f.optional {
                    wrong.push(format!(
                        "{at}: `{}` is {} in the shape but {} by the type",
                        f.name,
                        if f.optional { "optional" } else { "required" },
                        if left_out {
                            "may be left out"
                        } else {
                            "always written"
                        },
                    ));
                }
            }
        }
        assert!(checked > 0, "no component names its type");
        assert!(
            nested > 0,
            "no response type reads a struct below its top level"
        );
        assert!(
            wrong.is_empty(),
            "response shapes that differ from their types:\n{}",
            wrong.join("\n")
        );
    }

    /// `(status, code)` of every `ApiError` the body makes with a literal code.
    fn literal_codes(body: &str) -> Vec<(u16, String)> {
        let mut out = Vec::new();
        let literal = |s: &str| -> Option<String> {
            let s = s.trim_start().strip_prefix('"')?;
            Some(s[..s.find('"')?].to_owned())
        };
        let mut rest = body;
        while let Some(i) = rest.find("ApiError::new(") {
            rest = &rest[i + "ApiError::new(".len()..];
            let digits: String = rest
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            let after = rest.find(',').map(|c| &rest[c + 1..]);
            if let (Ok(status), Some(code)) = (digits.parse(), after.and_then(literal)) {
                out.push((status, code));
            }
        }
        for (call, status, code) in [
            ("bad_request(", 400, None),
            ("forbidden(", 403, Some("FORBIDDEN")),
            ("not_found(", 404, Some("NOT_FOUND")),
            ("conflict(", 409, Some("CONFLICT")),
        ] {
            let mut rest = body;
            while let Some(i) = rest.find(&format!("ApiError::{call}")) {
                rest = &rest[i + "ApiError::".len() + call.len()..];
                match code {
                    Some(c) => out.push((status, c.to_owned())),
                    None => out.extend(literal(rest).map(|c| (status, c))),
                }
            }
        }
        out
    }

    #[test]
    fn the_codes_a_handler_raises_are_listed() {
        let always = [
            "BAD_REQUEST",
            "REJECTED",
            "NOT_LEADER",
            "UNAVAILABLE",
            "INTERNAL",
        ];
        let mut wrong = Vec::new();
        for op in operations() {
            let (_, body) = handler(op.handler);
            let listed = op.codes();
            for (status, code) in literal_codes(&body) {
                let known = always.contains(&code.as_str())
                    || listed
                        .get(&status)
                        .is_some_and(|c| c.contains(code.as_str()));
                if !known {
                    wrong.push(format!("{} {}: {status} {code}", op.method, op.path));
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "codes handlers raise that their entries do not list:\n{}",
            wrong.join("\n")
        );
    }

    /// Values the node built of component `name`: each is a value of its schema, and over
    /// them a property is built at all, always when required, and not always when optional.
    fn value_mismatches(
        name: &str,
        values: &[&Value],
        shapes: &BTreeMap<&'static str, Shape>,
    ) -> Vec<String> {
        let refs = |n: &str| shapes[n].clone();
        let shape = &shapes[name];
        let mut wrong: Vec<String> = values
            .iter()
            .filter(|v| !shape.admits(v, &refs))
            .map(|v| format!("#{name} does not admit a value the node builds:\n    {v}"))
            .collect();
        for f in shape.fields() {
            let present = values.iter().filter(|v| v.get(&f.name).is_some()).count();
            let why = if present == 0 {
                "never built"
            } else if !f.optional && present < values.len() {
                "required, but left out of one"
            } else if f.optional && present == values.len() {
                "optional, but always built"
            } else {
                continue;
            };
            wrong.push(format!("#{name}: `{}` is {why}", f.name));
        }
        wrong
    }

    /// An error of every code `ERROR_DETAILS` names, as the node's own constructors build
    /// it, each optional property present in one and absent from another.
    fn built_errors() -> Vec<ApiError> {
        use crate::app::Status;
        use crate::runtime::NodeRuntime;
        use crate::system::cluster_version::{check, FEATURES};

        let confirm =
            || super::super::confirm_required("m", "1.id", &[7; 32], "rp", &["cred".to_owned()]);
        let min = semver::Version::new(1, 2, 0);
        let runtimes = [
            (1, NodeRuntime::Known(semver::Version::new(1, 1, 0))),
            (2, NodeRuntime::Before(semver::Version::new(1, 0, 0))),
            (3, NodeRuntime::Unknown),
        ]
        .into_iter()
        .collect();
        let short = crate::runtime::short_of(&[1, 2, 3], &runtimes, &min);
        let not_active = |active| {
            let refused = check(active, &FEATURES[..1]).expect_err("not active");
            ApiError::new(409, "FEATURE_NOT_ACTIVE", "m").with_details(refused.details())
        };
        let app_error = json!({ "code": "Nope", "fields": { "why": "x" } });
        let trap = crate::component::Outcome::trap(calimero_app_host::TrapKind::OutOfFuel, "d");
        let failure = |status: &Status, error: &Value, timeout: bool| {
            super::super::calls::component_failure(4, status, error, timeout).expect("a failure")
        };
        let bundle = |code: &str| ApiError::bad_request(code, "m");
        vec![
            confirm(),
            super::super::orgs::with_detail(confirm(), "apps", json!(["app"])),
            super::super::operator_signature_required(
                &crate::proof::Statement::new("cluster", "__set_quotas", &json!({ "a": 1 })),
                "c1",
            ),
            super::super::not_leader("m", Some(2), "https://n2"),
            super::super::not_leader("m", None, "https://n2"),
            not_active(None),
            not_active(Some("0.0.1")),
            crate::runtime::refusal("x", &min, short),
            crate::node::stalled(&crate::system::Stall {
                release: "1.0.0".to_owned(),
                after: 5,
            }),
            crate::node::upgrade_pending("x", "1.1.0", &[(1, Some("1.0.0".to_owned())), (2, None)]),
            super::super::upgrades::upgrade_failed("x", 3, &Status::AppError, &app_error),
            failure(&Status::AppError, &app_error, false),
            crate::node::app_refusal("UnknownMethod"),
            failure(&Status::Trap, &trap.error, false),
            failure(&Status::Ok, &Value::Null, true),
            super::super::apps::init_failed("app", &app_error),
            super::super::recovery::recovery_not_ready(9),
            crate::node::upload_refusal(409, "PART_CONFLICT", "m", 1),
            crate::node::upload_refusal(409, "PART_OUT_OF_ORDER", "m", 1),
            crate::node::upload_refusal(409, "UPLOAD_INCOMPLETE", "m", 1),
            crate::node::files_unavailable(),
            crate::packages::of_service(ApiError::unsupported_bundle(), "svc", false),
            crate::packages::of_service(bundle(crate::node::BAD_BUNDLE), "svc", true),
            crate::packages::of_service(
                bundle(crate::packages::COMPUTE_ABI_MISMATCH),
                "svc",
                false,
            ),
        ]
    }

    /// Each code's `details` is a value of the schema `ERROR_DETAILS` names for it, as the
    /// constructor that raises the code builds it; and over the values of one schema, a
    /// property is built at all, always when required, and not always when optional.
    #[test]
    fn error_details_match_their_constructors() {
        let shapes = shapes();
        let built = built_errors();
        let mut wrong = Vec::new();
        for e in &built {
            if !ERROR_DETAILS.iter().any(|(c, _)| *c == e.code) {
                wrong.push(format!("{}: no schema in ERROR_DETAILS", e.code));
            }
        }
        let names: BTreeSet<&str> = ERROR_DETAILS.iter().map(|(_, n)| *n).collect();
        for name in names {
            let codes: Vec<&str> = ERROR_DETAILS
                .iter()
                .filter(|(_, n)| *n == name)
                .map(|(c, _)| *c)
                .collect();
            for code in &codes {
                if !built.iter().any(|e| e.code == *code) {
                    wrong.push(format!("{code}: no constructor in built_errors"));
                }
            }
            let values: Vec<&Value> = built
                .iter()
                .filter(|e| codes.contains(&e.code.as_str()))
                .map(|e| &*e.details)
                .collect();
            wrong.extend(value_mismatches(name, &values, &shapes));
        }
        assert!(
            wrong.is_empty(),
            "error details that differ from their constructors:\n{}",
            wrong.join("\n")
        );
    }

    /// Components the node answers by serializing a type that does not deserialize, so
    /// `response_shapes_match_their_types` cannot read samples into it: real values of the
    /// type, each admitted by the schema and, over them, each property built.
    #[test]
    fn serialized_answers_match_their_shapes() {
        let status = |leader: Option<u64>, last_log: Option<u64>| {
            serde_json::to_value(calimero_log::Status {
                group: 1,
                leader,
                is_leader: leader == Some(1),
                term: 2,
                last_log,
                applied: 3,
                voters: vec![1, 2],
                learners: vec![3],
            })
            .expect("a status")
        };
        let rollback = serde_json::to_value(crate::rollback::Status {
            epoch: 1,
            generation: 2,
            mark: 3,
            forced: false,
            since_checkpoint_secs: 4,
        })
        .expect("a rollback status");
        let shapes = shapes();
        let mut wrong = Vec::new();
        for (name, values) in [
            (
                "GroupStatus",
                vec![status(Some(1), Some(4)), status(None, None)],
            ),
            ("RollbackStatus", vec![rollback]),
        ] {
            wrong.extend(value_mismatches(
                name,
                &values.iter().collect::<Vec<_>>(),
                &shapes,
            ));
        }
        assert!(
            wrong.is_empty(),
            "answers that differ from their schemas:\n{}",
            wrong.join("\n")
        );
    }

    /// `#QuotaChanges` names every limit `POST /ops/quotas` changes, the cluster's
    /// (`system::Quotas`) and a payer's (`system::OrgQuotas`), and each reads into its type.
    #[test]
    fn quota_changes_name_every_quota() {
        let shapes = shapes();
        let refs = |name: &str| shapes[name].clone();
        let shape = &shapes["QuotaChanges"];
        let cluster = fields_of::<crate::system::Quotas>().expect("a struct");
        let payer = fields_of::<crate::system::OrgQuotas>().expect("a struct");
        let named: BTreeSet<&str> = shape.fields().iter().map(|f| f.name.as_str()).collect();
        let limits: BTreeSet<&str> = cluster
            .iter()
            .chain(payer)
            .copied()
            .chain(["org", "operatorProof", "orgs"])
            .collect();
        assert_eq!(named, limits);
        let sample = shape.sample(&refs);
        let only = |names: &[&str]| -> Value {
            sample
                .as_object()
                .expect("an object")
                .iter()
                .filter(|(k, _)| names.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        let _cluster: crate::system::Quotas =
            serde_json::from_value(only(cluster)).expect("the cluster's limits");
        let _payer: crate::system::OrgQuotas =
            serde_json::from_value(only(payer)).expect("a payer's limits");
    }

    #[test]
    fn every_shape_parses_and_names_known_components() {
        let names: BTreeSet<&str> = components().iter().map(|c| c.name).collect();
        let doc = serde_json::to_string(&document()).expect("document");
        let mut rest = doc.as_str();
        while let Some(i) = rest.find("#/components/schemas/") {
            rest = &rest[i + "#/components/schemas/".len()..];
            let name = &rest[..rest.find('"').expect("ref end")];
            assert!(names.contains(name), "#{name} is not a component");
        }
    }

    /// The committed `docs/public/openapi.json` is what the registry builds now.
    /// `UPDATE_OPENAPI=1 cargo test -p calimero-node --lib openapi` rewrites it.
    #[test]
    fn the_committed_document_is_current() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/public/openapi.json");
        let built = format!(
            "{}\n",
            serde_json::to_string_pretty(&document()).expect("document")
        );
        if std::env::var_os("UPDATE_OPENAPI").is_some() {
            std::fs::write(&path, &built).expect("write openapi.json");
            return;
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        let first_difference = committed
            .lines()
            .zip(built.lines())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| committed.lines().count().min(built.lines().count()));
        assert!(
            committed == built,
            "{} is not what the registry in crates/node/src/api/openapi.rs builds: run \
             `UPDATE_OPENAPI=1 cargo test -p calimero-node --lib openapi` and commit it.\n\
             First difference, line {}:\n  committed: {}\n  built:     {}",
            path.display(),
            first_difference + 1,
            committed.lines().nth(first_difference).unwrap_or("<end>"),
            built.lines().nth(first_difference).unwrap_or("<end>"),
        );
        let parsed: Value = serde_json::from_str(&committed).expect("JSON");
        assert_eq!(parsed["openapi"], json!("3.1.0"));
    }
}
