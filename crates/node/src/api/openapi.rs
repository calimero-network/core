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

pub(super) mod generated;
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
            Self::AppAccess => "whoever the app's grants admit (`anyone`: any signed-in account)",
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
        // A session this node's release may not serve (`session_issuer_serves`).
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
    ("COMPILE_TIMEOUT", "ServiceRefused"),
    ("COMPUTE_ABI_MISMATCH", "ServiceRefused"),
    ("CONFIRM_REQUIRED", "ConfirmRequired"),
    ("FEATURE_NOT_ACTIVE", "FeatureNotActive"),
    ("FOLLOW_UP_FAILED", "FollowUpFailed"),
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
`x-calimero-sealed-refusal` names) carries `Retry-After: 1` on any sealed route, and so does \
`GET /attestation`'s `503 BUSY` (too many quotes at once); the node's `503 UNAVAILABLE` \
answers carry none. \
`x-calimero-step-up` (with `x-calimero-user-verification`, and `x-calimero-step-up-when` \
where it applies only sometimes) and `x-calimero-operator` mark \
the two-round operations below.\n\n\
**Numbers** that may exceed 2^53 (fuel, bytes) travel as decimal strings (D20); every \
other integer is `format: int64` (read it into a 64-bit integer), and stays below 2^53.";

/// The shared shapes.
fn components() -> Vec<Component> {
    vec![
        // ---------------------------------------------------------------- request bodies (E7)
        component(
            "AllAppsBody",
            "The request body of `POST /orgs/{org}/all-apps`.",
            "{ account: string, allApps: bool, confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "BackupKeyBody",
            "The request body of `POST /auth/backup-key`, `POST /orgs/{org}/backup-key`.",
            "{ publicKey: string, confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "CapabilitiesBody",
            "The request body of `POST /orgs/{org}/capabilities`.",
            "{ account?: string, team?: string, capabilities: uint, confirm?: #Confirm }",
        ),
        component(
            "DeployerBody",
            "The request body of `POST /ops/deployers`, `POST /deployers`.",
            "{ deployer: #Deployer, until?: string, operatorProof?: #OperatorProof, confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "DeploymentBody",
            "The request body of `POST /deployments`.",
            "{ name: string, owner: #Deployer, confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "DeploymentSecretBody",
            "The request body of `PUT /deployments/{id}/secrets/{name}`.",
            "{ value: string, confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "InviteBody",
            "The request body of `POST /orgs/{org}/invites`.",
            "{ role: #OrgRole, teams?: string[], ttlSecs?: uint, uses?: uint, allApps?: bool, capabilities?: uint, confirm?: #Confirm }",
        ),
        component(
            "MemberBody",
            "The request body of `POST /orgs/{org}/members`.",
            "{ account: string, role: #OrgRole, confirm?: #Confirm }",
        ),
        component(
            "MetadataBody",
            "The request body of `POST /apps/{app}/metadata`, `POST /orgs/{org}/metadata`, `POST /orgs/{org}/members/{account}/metadata`.",
            "{ metadata: any }",
        ),
        component(
            "OfferBody",
            "The request body of `POST /deployments/{id}/apps`.",
            "{ app: string, package: string, version: string, confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "OrgBody",
            "The request body of `POST /orgs`.",
            "{ name: string, recoveryKey?: string, capabilities?: uint }",
        )
        .closed_body(),
        component(
            "OrgSettingsBody",
            "The request body of `POST /orgs/{org}/settings`.",
            "{ allowGuests?: bool, allowPublicLinks?: bool, confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "OrgUpgradeBody",
            "The request body of `POST /orgs/{org}/upgrade`, `POST /ops/orgs/{org}/upgrade`.",
            "{ package: string, version: string, team?: string, cascade?: bool, migrate?: string, confirm?: #Confirm, operatorProof?: #OperatorProof }",
        ),
        component(
            "PublisherBody",
            "The request body of `POST /orgs/{org}/publishers`.",
            "{ signerId: string, publicKey: string, signature: string, confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "PublisherUpgradeBody",
            "The request body of `POST /orgs/{org}/packages/{package}/upgrade`.",
            "{ version: string, migrate?: string, confirm?: #Confirm }",
        ),
        component(
            "RecoveryKeyBody",
            "The request body of `POST /orgs/{org}/recovery`.",
            "{ publicKey?: string | null, confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "RecoveryStartBody",
            "The request body of `POST /orgs/{org}/recovery/start`.",
            "{ nonce: string, signature: string }",
        )
        .closed_body(),
        component(
            "RenameOrgBody",
            "The request body of `POST /orgs/{org}/name`.",
            "{ name: string, confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "ScopeAppsBody",
            "The request body of `POST /orgs/{org}/app-teams`.",
            "{ apps?: { package: string, name?: string, instances?: string[], join?: #TeamJoin, keep?: string[], childTeams?: bool }[], shared?: bool, confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "TeamBody",
            "The request body of `POST /orgs/{org}/teams`.",
            "{ name: string, parent?: string, join: #TeamJoin }",
        ),
        component(
            "TeamDeleteBody",
            "`DELETE /orgs/{org}/teams/{team}`'s body: whether the instances only the team admits stay, and the passkey confirmation after the `412`.",
            "{ keepApps?: bool, confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "TeamIdsBody",
            "The request body of `DELETE /orgs/{org}/members/{account}/teams`.",
            "{ teams: string[], confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "TeamMemberBody",
            "The request body of `POST /orgs/{org}/teams/{team}/members`.",
            "{ account: string, lead?: bool, role?: #TeamRole, capabilities?: uint, confirm?: #Confirm }",
        ),
        component(
            "TeamRowsBody",
            "The request body of `POST /orgs/{org}/members/{account}/teams`.",
            "{ teams: { team: string, role?: #TeamRole, capabilities?: uint }[], confirm?: #Confirm }",
        )
        .closed_body(),
        component(
            "TeamUpdateBody",
            "The request body of `POST /orgs/{org}/teams/{team}`.",
            "{ name?: string, join?: #TeamJoin, parent?: string | null, metadata?: any }",
        ),
        component(
            "UpgradePolicyBody",
            "A policy to set at one scope; `null` drops it (an app then follows its organization's, an organization the defaults).",
            "{ policy?: { autoApply?: \"never\" | \"noDestinations\", holdHours?: uint, secondApprover?: bool } | null, confirm?: #Confirm }",
        )
        .closed_body(),
        // ---------------------------------------------------------------- typed answers (E7)
        component(
            "ReplicaAttestation",
            "One replica's receipt-key attestation, or the error fetching it.",
            "{ node: uint, attestation: #AttestationDoc } | { node: uint, error: string }",
        )
        .of::<super::apps::ReplicaAttestation>(),
        component(
            "UseMode",
            "How a release holds an app it references (`uses`): `create`, made with the instance and upgraded with it as one unit; `attach`, an app its owner already runs, linked.",
            r#""create" | "attach""#,
        )
        .of::<calimero_bundle::UseMode>(),
        component(
            "AppAbi",
            "`GET /apps/{app}/abi`: the bundle the app group runs and the ABI it embeds (`null`: none).",
            "{ bundle: string, abi: any | null }",
        )
        .of::<super::apps::AppAbi>(),
        component(
            "AppAttestations",
            "`GET /apps/{app}/attestation`: the app, the cluster each replica's quote must bind, and each replica's receipt-key attestation (or why it could not be fetched).",
            "{ appId: string, cluster: string, replicas: #ReplicaAttestation[] }",
        )
        .of::<super::apps::AppAttestations>(),
        component(
            "AppCreated",
            "What `POST /apps` answers: the new instance, its context id, its group and replicas, whether its release ships a compute component, and the apps it created with it by name.",
            "{ appId: string, contextId: string, group: uint, replicas: uint[], compute: bool, links: { [string]: string } }",
        )
        .of::<super::apps::AppCreated>(),
        component(
            "AppDeleted",
            "What `DELETE /apps/{app}` answers: the app, and the members of its unit that went with it.",
            "{ appId: string, deleted: bool, members: string[] }",
        )
        .of::<super::apps::AppDeleted>(),
        component(
            "AppEvent",
            "An event a call emitted that the caller is in the audience of: its kind and data (the app's own value).",
            "{ kind: string, data: any }",
        )
        .of::<super::calls::AppEvent>(),
        component(
            "AppInviteList",
            "`GET /apps/{app}/invites`: the instance's invites.",
            "{ invites: #AppInviteView[] }",
        )
        .of::<super::apps::AppInviteList>(),
        component(
            "AppInviteMade",
            "What `POST /apps/{app}/invites` answers: the invite's token, shown once.",
            "{ token: string }",
        )
        .of::<super::apps::AppInviteMade>(),
        component(
            "AppInviteView",
            "An instance invite: its key, uses left, expiry, maker and the grant scope it carries.",
            "{ inviteKey: string, usesLeft: uint, expiresAt: uint, expired: bool, createdBy: string, createdAt: uint, scope: any | null }",
        )
        .of::<super::apps::AppInviteView>(),
        component(
            "AppLinkList",
            "`GET /apps/{app}/links`: each reference of the release the instance runs and what is linked.",
            "{ app: string, links: #LinkRow[] }",
        )
        .of::<super::apps::AppLinkList>(),
        component(
            "AppList",
            "`GET /apps`: the instances the caller reaches.",
            "{ apps: #AppEntry[] }",
        )
        .of::<super::apps::AppList>(),
        component(
            "AppMoved",
            "What `POST /apps/{app}/move` answers: where the app went, its access there, whether an app team was made, the accounts brought in and the invites revoked.",
            "{ appId: string, from: string, org: string, grants: #Grant[], created: bool, added: string[], revokedInvites: uint }",
        )
        .of::<crate::system::AppMoved>(),
        component(
            "AppOpenApi",
            "`GET /apps/{app}/openapi.json`: the app's OpenAPI document, generated from its ABI.",
            "{ [string]: any }",
        )
        .of::<super::apps::AppOpenApi>(),
        component(
            "AppRestored",
            "An app restored from a backup or a recovery file: its new group, the rows seeded, the backup it came from and the group's status once every replica holds it.",
            "{ app: string, group: uint, rows: uint, backup: { name: string | null, index: uint, format: uint, merod: string | null, release: string | null }, status: #GroupStatus }",
        )
        .of::<crate::node::restore::AppRestored>(),
        component(
            "AppSecrets",
            "`GET /apps/{app}/secrets`: each secret the release declares, by name.",
            "{ secrets: { [string]: { host: string, paths: string[], header?: string, format?: string, bodyField?: string, binding: string, description: string, required: bool, source: \"app\" | \"deployment\" | \"cluster\", fingerprint?: string, setBy?: string, setAt?: uint } } }",
        )
        .of::<super::apps::AppSecrets>(),
        component(
            "AppStorage",
            "`GET /apps/{app}/storage`: keys, bytes, blobs, and when it last committed a write (Unix seconds; `null` before its first).",
            "{ keys: uint, bytes: uint, blobs: uint, blobBytes: uint, lastWriteAt: uint | null }",
        )
        .of::<super::apps::AppStorage>(),
        component(
            "AppUsage",
            "`GET /apps/{app}/usage`: today, and the 30 days before it, oldest first.",
            "{ today: #UsageDay, days: #UsageDay[] }",
        )
        .of::<super::apps::AppUsage>(),
        component(
            "ApprovalLogged",
            "`POST /apps/{app}/upgrade-approvals`: the first approval logged.",
            "{ app: string, approval: #PendingApproval }",
        )
        .of::<super::update_policies::ApprovalLogged>(),
        component(
            "AppsReport",
            "`GET /orgs/{org}/apps` and `GET /auth/apps`: a page of one approval scope's apps, with the scope's approved releases and the releases the cluster's nodes run.",
            "{ scope: #AppScope, governance: #Governance, approvedReleases: string[] | null, clusterReleases: #ClusterRelease[], apps: #ReportApp[], next: string | null }",
        )
        .of::<super::AppsReport>(),
        component(
            "BackupWritten",
            "What `POST /ops/backup` answers: how many backups were written.",
            "{ written: uint }",
        )
        .of::<super::ops::BackupWritten>(),
        component(
            "CapabilityCard",
            "`GET /packages/{package}/{version}/capabilities`: a release's capability card (the release, its signer, outbound prefixes, secrets, `uses` and ABI, as the consent page shows each side of an upgrade), and where an instance of it could send data (`null`: a member release is not installed here).",
            "{ package: string, version: string, service: string | null, bundle: string, mpk: string, manifestHash: string, signer: string, name: string | null, author: string | null, outbound: string[], secrets: any, secretBindings: string[], uses: any, abi: any, destinations: string[] | null, members: { [string]: #UnitMember } | null }",
        )
        .of::<super::packages::CapabilityCard>(),
        component(
            "ClusterIdentity",
            "`GET /ops/cluster`: the cluster's name and identity key, which a job node is started with.",
            "{ cluster: string, identity: string }",
        )
        .of::<super::ops::ClusterIdentity>(),
        component(
            "ClusterOwnerNamed",
            "What `SetClusterOwner` answers: the owner named.",
            "{ owner: #Deployer }",
        )
        .of::<crate::system::ClusterOwnerNamed>(),
        component(
            "DelegationMade",
            "What `PUT /apps/{app}/delegations/{target}` answers: the methods the caller let `source` call on `target` as them.",
            "{ source: string, target: string, methods: string[] }",
        )
        .of::<super::apps::DelegationMade>(),
        component(
            "Delegations",
            "`GET /apps/{app}/delegations`: what the caller lets `source` do as them, by target.",
            "{ source: string, delegations: { [string]: string[] } }",
        )
        .of::<super::apps::Delegations>(),
        component(
            "EgressView",
            "`GET /apps/{app}/egress`: where the instance's unit may send data now (`destinations`, `members`), and whether its egress switch is on.",
            "{ app: string, destinations: string[], members: { [string]: #UnitMember }, egress: \"on\" | \"off\", offBy?: \"organization\" | \"instance\", summary?: string }",
        )
        .of::<super::upgrades::EgressView>(),
        component(
            "FileExport",
            "What `POST /apps/{app}/files/{id}/export` answers: the file's size, plaintext SHA-256 and segments, and its data key wrapped to the recipient (`ephemeral`, `wrapped`).",
            "{ app: string, id: string, size: uint, sha256: string, segments: string[], ephemeral: string, wrapped: string }",
        )
        .of::<super::exports::FileExport>(),
        component(
            "GovernanceSet",
            "What `SetDeploymentsGranted` answers: the cluster's governance after.",
            "{ governance: #Governance }",
        )
        .of::<crate::system::GovernanceSet>(),
        component(
            "HolderRecovered",
            "What `POST /recover/holder` answers: each app restored from the holder's files.",
            "{ apps: #AppRestored[] }",
        )
        .of::<super::restore::HolderRecovered>(),
        component(
            "IntentResult",
            "What a call (`/intents`) answers: the log index of its entry, the app's result, the events the caller receives, and the answering node's receipt.",
            "{ index: uint, result: any, events: #AppEvent[], receipt: #Receipt }",
        )
        .of::<super::calls::IntentResult>(),
        component(
            "JobNodeVerdict",
            "`GET /ops/job-nodes/{id}/attestation`: the leader's check of a job node.",
            "{ state: \"admitted\", node: uint, release: string } | { state: \"refused\" | \"unreachable\", node: uint, reason: string }",
        )
        .of::<super::ops::JobNodeVerdict>(),
        component(
            "LinkRow",
            "One link of an instance: the reference (`null` fields for one its release no longer names), the app linked, its release and whether it is in the reference's range (`null`: nothing linked).",
            "{ name: string, package: string | null, range: string | null, mode: #UseMode | null, app: string | null, version: string | null, inRange: bool | null }",
        )
        .of::<super::apps::LinkRow>(),
        component(
            "LinkView",
            "A link as its list shows it: its key and terms, never a token.",
            "{ link: string, label: string | null, expiresAt: uint | null, expired: bool, ratePerMinute: uint, createdBy: string, createdAt: uint }",
        )
        .of::<super::public_links::LinkView>(),
        component(
            "MethodResult",
            "What `POST /apps/{app}/methods/{method}` answers: a call's answer, or a view's (no `events`), as the method is. The handler answers one of [`IntentResult`] and [`ViewResult`]; this is their schema together, for the API's document.",
            "{ index: uint, result: any, events?: #AppEvent[], receipt: #Receipt }",
        )
        .of::<super::calls::MethodResult>(),
        component(
            "NodeRemoved",
            "What `DELETE /ops/nodes/{id}` answers: the system group's status after.",
            "{ status: #GroupStatus }",
        )
        .of::<super::ops::NodeRemoved>(),
        component(
            "OpsApps",
            "`GET /ops/apps`: a page of what every app runs, for the operator.",
            "{ fresh: bool, governance: #Governance, apps: #ReportApp[], next: string | null }",
        )
        .of::<super::ops::OpsApps>(),
        component(
            "OpsOrgs",
            "`GET /ops/orgs`: a page of organizations and the releases approved to hold their apps.",
            "{ fresh: bool, governance: #Governance, orgs: { org: string, approvedReleases: string[] | null }[], next: string | null }",
        )
        .of::<super::ops::OpsOrgs>(),
        component(
            "OpsPools",
            "`GET /ops/pools`: the pools, and the runs each takes at once.",
            "{ pools: { [string]: { runsPerNode: uint, maxMemoryMib: uint } }, capacity: { [string]: uint } }",
        )
        .of::<super::ops::OpsPools>(),
        component(
            "OpsStatus",
            "`GET /ops/status`: this node's view of the cluster: its system group, governance, the app groups it hosts, the registered nodes, counts, and its rollback counter (`null`: unprotected).",
            "{ node: uint, system: #GroupStatus, governance: #Governance, groups: { app: string, status: #GroupStatus }[], nodes: #NodeRecord[], apps: uint, accounts: uint, rollback: #RollbackStatus | null }",
        )
        .of::<super::ops::OpsStatus>(),
        component(
            "PipelineCancelled",
            "What `POST /apps/{app}/pipeline-runs/{run}/cancel` answers: the run cancelled.",
            "{ run: string }",
        )
        .of::<super::pipelines::PipelineCancelled>(),
        component(
            "PipelineRunList",
            "`GET /apps/{app}/pipeline-runs`: the app's recent pipeline runs, newest first.",
            "{ runs: #PipelineRunView[] }",
        )
        .of::<super::pipelines::PipelineRunList>(),
        component(
            "PipelineRunView",
            "A pipeline run as the app's leader shows it: its stored record (`PipelineRun`), reshaped for the reader (results and file hashes only to those who handle the app's files). Its keys follow the record's, so the document leaves it open.",
            "{ [string]: any }",
        )
        .of::<super::pipelines::PipelineRunView>(),
        component(
            "PipelineStarted",
            "What a pipeline run's start (and retry) answers: its id, its worst-case cost under a cap, and the run it retries.",
            "{ run: string, worstCase?: string, retryOf?: string }",
        )
        .of::<super::pipelines::PipelineStarted>(),
        component(
            "PlacedApp",
            "An app's group and replica nodes.",
            "{ app: string, group: uint, replicas: uint[] }",
        )
        .of::<super::ops::PlacedApp>(),
        component(
            "PlacedNode",
            "A node's zone, role, attested release (`null`: unknown) and how many apps it holds.",
            "{ id: uint, zone: string, role: #NodeRole, release: string | null, apps: uint }",
        )
        .of::<super::ops::PlacedNode>(),
        component(
            "Placement",
            "`GET /ops/placement`: where every app's replicas are, and each node's zone, role, attested release and app count; ids and counts only.",
            "{ fresh: bool, apps: #PlacedApp[], nodes: #PlacedNode[], clusterVersion: string | null }",
        )
        .of::<super::ops::Placement>(),
        component(
            "PresenceDiff",
            "A change a stream reports: an author's new state, or `None` when they left.",
            "{ account: string, device: string, state: string | null }",
        )
        .of::<crate::presence::PresenceDiff>(),
        component(
            "PresenceList",
            "`GET /apps/{app}/presence`: who is here, on which device, and their state (`null`: none set).",
            "{ present: #PresenceDiff[] }",
        )
        .of::<super::calls::PresenceList>(),
        component(
            "PriceTableView",
            "`GET /ops/prices`: the price table, every field absent until one is set.",
            "{ version?: uint, fuelPerVcpuSecond?: uint, vcpuSecond?: uint, gibSecond?: uint, connectorGib?: uint, storageGibMonth?: uint, egressGib?: uint }",
        )
        .of::<super::ops::PriceTableView>(),
        component(
            "PublicLinkMade",
            "`POST /apps/{app}/public-links`: the link's key and, this once, its token.",
            "{ link: string, token: string }",
        )
        .of::<super::public_links::PublicLinkMade>(),
        component(
            "PublicLinks",
            "`GET /apps/{app}/public-links`.",
            "{ links: #LinkView[] }",
        )
        .of::<super::public_links::PublicLinks>(),
        component(
            "ReceiptKey",
            "`GET /ops/nodes/{id}/receipt-key`: the receipt key this node holds for node `id`, the release it attested (`null`: unknown) and when.",
            "{ node: uint, key: string, release: string | null, attestedAt: uint }",
        )
        .of::<super::account_proof::ReceiptKey>(),
        component(
            "RecoveryFiles",
            "`GET /ops/recovery-files`: a holder and its recovery files, each with its signed header.",
            "{ holder: { kind: \"org\", id: string } | { kind: \"account\", id: string } | { kind: \"operator\" }, files: { name: string, header: string, signature: string }[] }",
        )
        .of::<super::restore::RecoveryFiles>(),
        component(
            "ReleasePipeline",
            "A registered pipeline: its hash, worst-case fuel (a decimal string), the module releases it pins, and its canonical JSON.",
            "{ hash: string, worstCaseFuel: string, modules: { [string]: { package: string, version: string, compute: string } }, pipeline: any }",
        )
        .of::<crate::packages::pipelines::ReleasePipeline>(),
        component(
            "ReleasePipelines",
            "`GET /packages/{package}/{version}/pipelines`: the release's pipelines by name.",
            "{ [string]: #ReleasePipeline }",
        )
        .of::<crate::packages::pipelines::ReleasePipelines>(),
        component(
            "ReplicasSet",
            "What `POST /ops/apps/{app}/replicas` answers: the app group's status after, and whether the set was already the one asked for.",
            "{ status: #GroupStatus, unchanged: bool }",
        )
        .of::<super::ops::ReplicasSet>(),
        component(
            "RunEstimate",
            "What `POST /apps/{app}/pipelines/{name}/estimate` answers: per step its runs and fuel, the whole run's fuel, and its cost and the price table's version when the cluster has one; every figure a decimal string.",
            "{ steps: { [string]: #StepEstimate }, fuel: string, cost?: string, priceVersion?: uint }",
        )
        .of::<super::pipelines::RunEstimate>(),
        component(
            "RunList",
            "`GET /apps/{app}/runs`: a page of the app's compute runs, newest first, and the cursor of the next page (`null`: none).",
            "{ runs: #Run[], next: string | null }",
        )
        .of::<crate::node::RunList>(),
        component(
            "RunLogs",
            "`GET /apps/{app}/runs/{run}/logs` (deprecated): always empty, a run keeps no log.",
            "{ logs: { attempt: uint, id: string, size: uint, sha256: string }[] }",
        )
        .of::<super::calls::RunLogs>(),
        component(
            "SecretCleared",
            "What `DELETE /apps/{app}/secrets/{name}` answers: the app log's entry.",
            "{ name: string, index: uint }",
        )
        .of::<super::apps::SecretCleared>(),
        component(
            "SecretSet",
            "What `PUT /apps/{app}/secrets/{name}` answers: the value's fingerprint and the app log's entry.",
            "{ name: string, fingerprint: string, index: uint }",
        )
        .of::<super::apps::SecretSet>(),
        component(
            "ServedOrigins",
            "`GET /ops/served-origins`: the origins the operator serves frontends on, each with its host, its package's label and when it was added (RFC 3339).",
            "{ fresh: bool, origins: { origin: string, host: string, label: string, addedAt: string }[] }",
        )
        .of::<super::origins::ServedOrigins>(),
        component(
            "SignedPipelineReceipt",
            "`GET /apps/{app}/pipeline-runs/{run}/receipt`: a pipeline run's receipt (`calimero_client::receipt::PipelineReceipt`) with the signature of the answering node's receipt key over it.",
            "{ receipt: any, node: uint, signature: string }",
        )
        .of::<crate::node::SignedPipelineReceipt>(),
        component(
            "StepEstimate",
            "One step's estimate: its runs, fuel and (with a price table) cost, as decimal strings.",
            "{ runs: string, fuel: string, cost?: string }",
        )
        .of::<super::pipelines::StepEstimate>(),
        component(
            "SystemRecovered",
            "What `POST /ops/recover` answers: the apps and accounts the system state was founded with again, and the index of the `system` file it came from.",
            "{ apps: uint, accounts: uint, index: uint }",
        )
        .of::<crate::node::restore::SystemRecovered>(),
        component(
            "UndoDone",
            "What `POST /apps/{app}/undo` answers: each app group restored (its restore point's index, `null` when none), whether every group takes calls again, and each app reverted.",
            "{ app: string, groups: { [string]: uint | null }, restored: bool, apps: #UndoPart[] }",
        )
        .of::<super::undo::UndoDone>(),
        component(
            "UndoView",
            "`GET /apps/{app}/undo`: whether the app's last upgrade may be undone now (else the refusal's `reason`), until when, and each app it reverts.",
            "{ app: string, available: bool, reason: { code: string, message: string } | null, undoUntil?: uint, apps: #UndoPart[] }",
        )
        .of::<super::undo::UndoView>(),
        component(
            "UpdateView",
            "`GET /apps/{app}/update`: the release the instance runs, the update available to it (with the classifier's verdict), who may approve it, the policy in force and the live first approvals.",
            "{ app: string, running: { package: string, version: string } | null, available: { package: string, version: string, source: \"deployment\" | \"release\", class?: \"green\" | \"red\", reasons?: #UpdateReason[], egress?: { before: string[], after: string[], added: string[], removed: string[] } | null, stateVersionChanged?: bool | null, newMethods?: string[] | null, removedMethods?: string[] | null, signerChanged?: bool | null, summary?: string, approvalsNeeded: uint, approvedBy?: string } | null, unit: bool, mayApprove: bool, approvers: \"owner-and-admins\" | \"operator\" | \"managers\", policy: { autoApply: \"never\" | \"noDestinations\", holdHours: uint, secondApprover: bool, source: \"app\" | \"org\" | \"default\" }, pendingApprovals: #PendingApproval[], deployment: #DeploymentShown | null }",
        )
        .of::<super::upgrades::UpdateView>(),
        component(
            "UpgradeApplied",
            "What `POST /apps/{app}/upgrade` answers: the app log's entry, how it ended, the app's error (`null` when it ended `ok`) and whether the release ships a compute component.",
            "{ index: uint, status: \"ok\" | \"appError\" | \"trap\", error: any, compute: bool }",
        )
        .of::<super::upgrades::UpgradeApplied>(),
        component(
            "ViewResult",
            "What a view (`/views`) answers: the log index it read at, the app's result, and the answering node's receipt.",
            "{ index: uint, result: any, receipt: #Receipt }",
        )
        .of::<super::calls::ViewResult>(),
        component(
            "XcallsReport",
            "`GET /apps/{app}/xcalls`: messages waiting, and those never delivered.",
            "{ pending: uint, deadLetters: #DeadLetter[] }",
        )
        .of::<crate::component::XcallsReport>(),
        component(
            "BlobList",
            "`GET /apps/{app}/blobs`.",
            "{ blobs: #BlobView[] }",
        )
        .of::<super::files::BlobList>(),
        component(
            "BlobStored",
            "A blob stored, and the index of the app's log entry that holds it.",
            "{ blobId: string, id: string, size: uint, index: uint }",
        )
        .of::<super::files::BlobStored>(),
        component(
            "BlobView",
            "A blob of an app: its id (base58 `blobId` and hex `id`) and size.",
            "{ blobId: string, id: string, size: uint }",
        )
        .of::<super::files::BlobView>(),
        component(
            "FileList",
            "`GET /apps/{app}/files`.",
            "{ files: #FileView[] }",
        )
        .of::<super::files::FileList>(),
        component(
            "FileStored",
            "A sealed file stored, and the index of the app's log entry that holds it.",
            "{ blobId: string, id: string, size: uint, sha256: string, index: uint }",
        )
        .of::<super::files::FileStored>(),
        component(
            "FileView",
            "A sealed file of an app: its ids, plaintext size and SHA-256 (hex), never its key.",
            "{ blobId: string, id: string, size: uint, sha256: string }",
        )
        .of::<super::files::FileView>(),
        component(
            "Logged",
            "A change logged on the app: the index of its entry.",
            "{ index: uint }",
        )
        .of::<super::files::Logged>(),
        component(
            "PartTaken",
            "A part taken: the part the session expects next.",
            "{ next: uint }",
        )
        .of::<crate::node::PartTaken>(),
        component(
            "UploadStatus",
            "An upload session's status: the file's size and the part it expects next.",
            "{ uploadId: string, partBytes: uint, parts: uint, expiresAt: uint, size: uint, next: uint }",
        )
        .of::<crate::node::UploadStatus>(),
        component(
            "UploadView",
            "An upload session: its id, the size of each part (the last may be shorter), how many parts and when it lapses unless a part arrives (Unix seconds).",
            "{ uploadId: string, partBytes: uint, parts: uint, expiresAt: uint }",
        )
        .of::<crate::node::UploadView>(),
        component(
            "VersionDeadline",
            "A mero-tee release's end-of-new-work deadline: `after` RFC 3339, `afterSecs` Unix seconds.",
            "{ version: string, after: string, afterSecs: uint }",
        )
        .of::<crate::system::VersionDeadline>(),
        component(
            "BundleStored",
            "`POST /bundles`: the stored WASM's hash (development only).",
            "{ hash: string }",
        )
        .of::<super::packages::BundleStored>(),
        component(
            "ClusterReleases",
            "`GET /releases`: the cluster's governance and the deadlines set on mero-tee releases, oldest release first.",
            "{ governance: #Governance, deadlines: #VersionDeadline[] }",
        )
        .of::<super::approvals::ClusterReleases>(),
        component(
            "ConsentRequestMade",
            "`POST /consent/requests`: the request, the page to send the person to, when it lapses and the code the page shows.",
            "{ requestId: string, url: string, code: string, expiresAt: uint }",
        )
        .of::<crate::consent_page::ConsentRequestMade>(),
        component(
            "ConsentResult",
            "`GET /consent/{request}/result`: the request's decision (`decidedAt` `null` while it is pending) and, once approved, what the approval made.",
            "{ requestId: string, kind: \"release\" | \"upgrade\" | \"upgradeUnit\" | \"delegate\" | \"outbound\" | \"createOrg\" | \"createInvite\" | \"acceptInvite\" | \"leaveOrg\" | \"removeMember\" | \"setMemberRole\" | \"setMemberTeams\" | \"removeMemberTeams\" | \"revokeInvite\" | \"renameOrg\" | \"deleteOrg\" | \"setCapabilities\" | \"setAllApps\" | \"setEgress\" | \"install\" | \"updatePolicy\" | \"updates\", status: \"pending\" | \"approved\" | \"declined\", decidedAt: uint | null, outcome?: { orgId?: string, teamId?: string, token?: string, org?: string, name?: string, apps?: string[], appId?: string, links?: { [string]: string } } }",
        )
        .of::<crate::consent_page::ConsentResult>(),
        component(
            "Health",
            "`GET /health`, in the clear: the node, the system group's leader (`null`: none known), its release, the newest cluster version it supports and the cluster's rollback epoch (`null`: a node without a rollback counter).",
            "{ ok: bool, node: uint, systemLeader: uint | null, release: string, clusterVersion: string, rollbackEpoch: uint | null }",
        )
        .of::<super::Health>(),
        component(
            "InviteRedeemed",
            "What redeeming an invite answers: the organization joined; for an instance invite, the instance and whether the account is a member of its organization (else a guest).",
            "{ app?: string, org: string, member?: bool }",
        )
        .of::<crate::system::InviteRedeemed>(),
        component(
            "OriginList",
            "`GET /package-origins/{package}`: the origins registered for a package.",
            "{ package: string, origins: #PackageOrigin[] }",
        )
        .of::<super::origins::OriginList>(),
        component(
            "OriginRemoved",
            "What removing an origin answers.",
            "{ package: string, origin: string, removed: bool }",
        )
        .of::<crate::origins::OriginRemoved>(),
        component(
            "PackageInstalled",
            "A release installed: its package, version, pinned signer and services (name to bundle hash).",
            "{ package: string, version: string, signerId: string, services: { [string]: string } }",
        )
        .of::<crate::packages::PackageInstalled>(),
        component(
            "PackageList",
            "The packages the caller may see.",
            "{ packages: #Package[] }",
        )
        .of::<super::packages::PackageList>(),
        component(
            "PipelineError",
            "One error in a pipeline: where (a path into it, empty for the whole), its code and what is wrong.",
            "{ path: string, code: string, message: string }",
        )
        .of::<crate::packages::pipelines::PipelineError>(),
        component(
            "PipelineValidation",
            "`POST /pipelines/validate`: whether the pipeline compiles (and, against a release, checks), every error, its canonical JSON, and its worst-case fuel and cost (decimal strings).",
            "{ ok: bool, errors: #PipelineError[], pipeline?: any, worstCaseFuel?: string, worstCaseCost?: string }",
        )
        .of::<crate::packages::pipelines::PipelineValidation>(),
        component(
            "PublisherBinding",
            "Which organization publishes the packages a signing key signs.",
            "{ signerId: string, cluster: string, generation: uint, bound: bool }",
        )
        .of::<super::packages::PublisherBinding>(),
        component(
            "SignUpInviteList",
            "The cluster's sign-up policy and live invites (keys and terms, never a token), and who runs sign-up (`owner` or `operator`).",
            "{ signUp: #SignUpState, runBy: \"owner\" | \"operator\", invites: #SignUpInvite[] }",
        )
        .of::<super::sign_up::SignUpInviteList>(),
        component(
            "SignUpInviteMade",
            "A sign-up invite the owner just made: its terms, then its token, answered this once.",
            "{ inviteKey: string, uses: uint, expiresAt: string, expiresAtSecs: uint, createdBy: string, createdAt: string, token: string }",
        )
        .of::<super::sign_up::SignUpInviteMade>(),
        component(
            "SignUpPublic",
            "`GET /sign-up`: the policy and what it implies, the cluster's name and the tag a sign-in uses to pick the person's passkey for it.",
            "{ policy: #SignUp, orgInvites: bool, signUpInvites: bool, inviteRequired: bool, cluster: string, passkeyTag: string }",
        )
        .of::<super::sign_up::SignUpPublic>(),
        component(
            "AccountOrg",
            "One organization of an [`AccountView`]: the caller's role, and whether it is a personal one.",
            "{ orgId: string, name: string, role: #OrgRole, personal: bool }",
        )
        .of::<super::auth::AccountOrg>(),
        component(
            "AccountPasskey",
            "One passkey of an [`AccountView`]: its label, and when it was added and last signed in (`null`: before the node kept it).",
            "{ id: string, signCount: uint, label: string | null, addedAt: uint | null, lastSignInAt: uint | null }",
        )
        .of::<super::auth::AccountPasskey>(),
        component(
            "AccountRecoveryPending",
            "An account recovery under way: the passkey it would add, when it started and when it may finish (Unix seconds).",
            "{ accountId: string, credentialId: string, startedAt: uint, readyAt: uint }",
        )
        .of::<crate::types::AccountRecoveryPending>(),
        component(
            "AccountUsage",
            "`GET /auth/usage`: what the account uses against the quotas, and its developer usage today (64-bit sums as decimal strings).",
            "{ apps: uint, bundleBytes: u64, apiKeys: uint, orgsOwned: uint, fileBytes: { [string]: uint }, quotas: #Quotas, developerFuelToday: u64, developerWritesToday: u64 }",
        )
        .of::<super::auth::AccountUsage>(),
        component(
            "AccountView",
            "`GET /auth/me`: the caller's account, its passkeys and organizations, how the caller came and its recovery state.",
            "{ accountId: string, name: string, passkeys: #AccountPasskey[], orgs: #AccountOrg[], via: \"session\" | \"apiKey\", passkeyId: string | null, app: string | null, origin: string | null, recoveryCode: bool, recoveryPending: #AccountRecoveryPending | null }",
        )
        .of::<super::auth::AccountView>(),
        component(
            "ActivityList",
            "`GET /auth/activity`: what the account's passkeys did, newest first.",
            "{ activity: #ActivityView[] }",
        )
        .of::<super::auth::ActivityList>(),
        component(
            "ActivityView",
            "One activity entry: a sign-in (its package and origin) or an approval (its method and subject), every key present, those its kind does not have `null`.",
            "{ at: uint, passkey: string, kind: \"signIn\" | \"approval\", app: string | null, origin: string | null, method: string | null, subject: string | null }",
        )
        .of::<super::auth::ActivityView>(),
        component(
            "AliasList",
            "`GET /auth/aliases`: the account's names for apps.",
            "{ aliases: { [string]: string } }",
        )
        .of::<super::auth::AliasList>(),
        component(
            "AliasView",
            "`GET /auth/aliases/{alias}`.",
            "{ alias: string, appId: string }",
        )
        .of::<super::auth::AliasView>(),
        component(
            "ApiKeyList",
            "`GET /auth/apikeys`: the account's keys, never a key itself.",
            "{ keys: #ApiKey[] }",
        )
        .of::<super::auth::ApiKeyList>(),
        component(
            "ApiKeyMade",
            "`POST /auth/apikeys`: the key, answered once, and what the calls made with it answered.",
            "{ keyId: string, key: string, then: { index: uint, result: any, receipt: #Receipt }[] }",
        )
        .of::<super::auth::ApiKeyMade>(),
        component(
            "PasskeyAdded",
            "A passkey added to the account.",
            "{ credentialId: string }",
        )
        .of::<super::auth::PasskeyAdded>(),
        component(
            "PasskeyChallenge",
            "A WebAuthn ceremony begun: its challenge id, and the options to hand the browser as they are (`navigator.credentials.create` or `.get`; WebAuthn's shape, not this API's).",
            "{ challengeId: string, options: any }",
        )
        .of::<super::auth::PasskeyChallenge>(),
        component(
            "RecoveryCode",
            "`POST /auth/recovery`: the account's recovery code, shown once.",
            "{ code: string }",
        )
        .of::<super::recovery::RecoveryCode>(),
        component(
            "SessionList",
            "`GET /auth/sessions`: the account's live sessions, latest active first.",
            "{ sessions: #SessionView[] }",
        )
        .of::<super::auth::SessionList>(),
        component(
            "SessionView",
            "One live session: its passkey, the package and origin it is bound to, when it opened (`null`: before the node kept it), when it was last active and whether it is the caller's.",
            "{ session: string, passkey: string | null, app: string | null, origin: string | null, openedAt: uint | null, lastActiveAt: uint, current: bool }",
        )
        .of::<super::auth::SessionView>(),
        component(
            "SessionsRevoked",
            "What `RevokeAllSessions` answers: how many sessions ended.",
            "{ revoked: uint }",
        )
        .of::<crate::system::SessionsRevoked>(),
        component(
            "AppOffered",
            "What `OfferApp` answers: the offer now in force.",
            "{ deployment: string, app: string, offer: #Offer }",
        )
        .of::<crate::system::AppOffered>(),
        component(
            "AppTeamList",
            "`GET /orgs/{org}/app-teams`: the organization's mode and its app teams.",
            "{ mode: \"legacy\" | \"standalone\" | \"shared\", appTeams: #AppTeamRow[] }",
        )
        .of::<super::orgs::AppTeamList>(),
        component(
            "AppTeamsScoped",
            "What `ScopeApps` answers: the organization's mode after, and each package's app team.",
            "{ mode: \"legacy\" | \"standalone\" | \"shared\", teams: #ScopedTeam[] }",
        )
        .of::<crate::system::AppTeamsScoped>(),
        component(
            "AppWithdrawn",
            "What `WithdrawApp` answers.",
            "{ deployment: string, app: string }",
        )
        .of::<crate::system::AppWithdrawn>(),
        component(
            "DeployerGranted",
            "What `GrantDeployer` answers: the grant's key, whom it names and its terms.",
            "{ key: string, deployer: #Deployer, grant: #DeployerGrant }",
        )
        .of::<crate::system::DeployerGranted>(),
        component(
            "DeployerList",
            "`GET /deployers`: the cluster's deployment rules, its owner and every grant.",
            "{ deployments: \"open\" | \"granted\", deployers: \"operator\" | \"owner\", owner: #Deployer | null, grants: #DeployerGrantView[] }",
        )
        .of::<super::deployments::DeployerList>(),
        component(
            "DeployerRevoked",
            "What `RevokeDeployer` answers: the key of the grant withdrawn.",
            "{ key: string }",
        )
        .of::<crate::system::DeployerRevoked>(),
        component(
            "DeploymentCreated",
            "`POST /deployments`: the record the log made, and its frontend host.",
            "{ id: string, deployment: #DeploymentRecord, host: string | null }",
        )
        .of::<super::deployments::DeploymentCreated>(),
        component(
            "DeploymentDeleted",
            "What `DeleteDeployment` answers.",
            "{ deployment: string }",
        )
        .of::<crate::system::DeploymentDeleted>(),
        component(
            "DeploymentList",
            "`GET /deployments`.",
            "{ deployments: #Deployment[] }",
        )
        .of::<super::deployments::DeploymentList>(),
        component(
            "DeploymentOrgs",
            "`GET /deployments/{id}/orgs`: the customer organizations, by counts only.",
            "{ deployment: string, orgs: #DeploymentCustomer[], outsideOrgs: uint }",
        )
        .of::<super::deployments::DeploymentOrgs>(),
        component(
            "DeploymentSecretCleared",
            "What `ClearDeploymentSecret` answers.",
            "{ deployment: string, name: string }",
        )
        .of::<crate::system::DeploymentSecretCleared>(),
        component(
            "DeploymentSecretSet",
            "What `SetDeploymentSecret` answers: never the value, its fingerprint.",
            "{ deployment: string, name: string, fingerprint: string, setBy: string, setAt: uint }",
        )
        .of::<crate::system::DeploymentSecretSet>(),
        component(
            "DeploymentSecrets",
            "`GET /deployments/{id}/secrets`: each name its offers declare or it holds a value for.",
            "{ deployment: string, secrets: { [string]: #DeploymentSecretView } }",
        )
        .of::<super::deployments::DeploymentSecrets>(),
        component(
            "EgressSet",
            "What `SetEgress` answers: the switch as set, the organization's own, and the instance when one was named.",
            "{ org: string, app?: string, egress: bool, orgEgressOff: bool }",
        )
        .of::<crate::system::EgressSet>(),
        component(
            "InviteList",
            "`GET /orgs/{org}/invites`.",
            "{ invites: #InviteView[] }",
        )
        .of::<super::orgs::InviteList>(),
        component(
            "InviteMade",
            "`POST /orgs/{org}/invites`: the invite's token, answered once.",
            "{ token: string }",
        )
        .of::<super::orgs::InviteMade>(),
        component(
            "OrgCreated",
            "What `CreateOrg` answers: the new organization, and its app team when it is made with one (the consent page's `createOrg` for a package).",
            "{ orgId: string, teamId?: string }",
        )
        .of::<crate::system::OrgCreated>(),
        component(
            "OrgDeleted",
            "What `DeleteOrg` answers: the instances deleted with it.",
            "{ apps: string[] }",
        )
        .of::<crate::system::OrgDeleted>(),
        component(
            "MemberSecurityList",
            "`GET /orgs/{org}/members?detail=security`: each member the caller sees, with what tells whether their account is safe.",
            "{ orgId: string, members: #MemberSecurity[] }",
        )
        .of::<super::orgs::MemberSecurityList>(),
        component(
            "OrgRenamed",
            "What `RenameOrg` answers.",
            "{ orgId: string, name: string }",
        )
        .of::<crate::system::OrgRenamed>(),
        component(
            "OrgSettingsSet",
            "What `SetOrgSettings` answers: the settings after the change.",
            "{ org: string, allowGuests: bool, allowPublicLinks: bool }",
        )
        .of::<crate::system::OrgSettingsSet>(),
        component(
            "OrgUpdates",
            "`GET /orgs/{org}/updates`: a page of its apps with an update or a pending approval, each as `GET /apps/{app}/update` shows it with its `name`, and the organization's policy.",
            "{ org: string, policy: #UpgradePolicy, updates: any[], next: string | null }",
        )
        .of::<super::update_policies::OrgUpdates>(),
        component(
            "OrgUsage",
            "`GET /orgs/{org}/usage`: the apps billed to it, its developer usage today (decimal strings) and the cluster's quotas.",
            "{ apps: uint, developerFuelToday: u64, developerWritesToday: u64, quotas: #Quotas }",
        )
        .of::<super::orgs::OrgUsage>(),
        component(
            "PublicDeployment",
            "`GET /deployments/{id}`, public: its name, whose it is, and what it offers.",
            "{ id: string, host: string | null, name: string, owner: #DeploymentOwner, apps: #PublicDeploymentApp[] }",
        )
        .of::<super::deployments::PublicDeployment>(),
        component(
            "PublisherBound",
            "What `BindPublisher` answers: the key's binding generation now.",
            "{ generation: uint }",
        )
        .of::<crate::system::PublisherBound>(),
        component(
            "RecoveryFinished",
            "What an organization recovery's finish answers: the organization and the owners it replaced.",
            "{ org: string, previousOwners: string[] }",
        )
        .of::<crate::system::RecoveryFinished>(),
        component(
            "RecoveryStarted",
            "What an organization recovery's start answers: when it started and when it may finish (Unix seconds).",
            "{ startedAt: uint, readyAt: uint }",
        )
        .of::<crate::system::RecoveryStarted>(),
        component(
            "TeamCreated",
            "What `CreateTeam` answers.",
            "{ teamId: string }",
        )
        .of::<crate::system::TeamCreated>(),
        component(
            "TeamDeleted",
            "What `DeleteTeam` answers: the teams deleted (the team and those below it), the instances deleted with them, and with `keepApps` those kept instead (`apps` then empty).",
            "{ teams: string[], apps: string[], keptApps?: string[] }",
        )
        .of::<crate::system::TeamDeleted>(),
        component(
            "AppTeamRow",
            "One app team: its package, how many are in it, the capabilities a new member gets (`null`: the organization's), and whether the caller is in it or manages it.",
            "{ package: string, teamId: string, name: string, join: #TeamJoin, memberCount: uint, defaultCapabilities: uint | null, member: bool, manages: bool }",
        )
        .of::<super::orgs::AppTeamRow>(),
        component(
            "CustomerApp",
            "An app a customer runs from the deployment, and its version.",
            "{ app: string, version: string }",
        )
        .of::<super::deployments::CustomerApp>(),
        component(
            "DeployerGrantView",
            "A deployer grant: whom it names (`null`: a key no deployer parses), until when (`until` RFC 3339, `untilSecs` Unix seconds; `null`: no end), who granted it and when.",
            "{ key: string, deployer: #Deployer | null, until: string | null, untilSecs: uint | null, grantedAt: string, grantedBy: string }",
        )
        .of::<super::deployments::DeployerGrantView>(),
        component(
            "DeploymentCustomer",
            "A customer organization of a deployment, by counts only: its members, those active in the last 30 days, its instances and the releases they run, and when its first was made (RFC 3339).",
            "{ orgId: string, name: string | null, memberCount: uint, activeMembers30d: uint, instances: uint, apps: #CustomerApp[], firstCreatedAt: string }",
        )
        .of::<super::deployments::DeploymentCustomer>(),
        component(
            "DeploymentOffer",
            "An app a deployment offers, as its managers list it: the release, who offered it and when (RFC 3339).",
            "{ app: string, package: string, version: string, addedBy: string, addedAt: string }",
        )
        .of::<super::deployments::DeploymentOffer>(),
        component(
            "DeploymentOwner",
            "Whose deployment it is: an account or an organization, and its name (`null`: gone).",
            "{ kind: \"account\" | \"org\", name: string | null }",
        )
        .of::<super::deployments::DeploymentOwner>(),
        component(
            "DeploymentSecretView",
            "A secret of a deployment, never its value: the offered apps and releases that declare it, whether a value is set, and its fingerprint, setter and time when it is.",
            "{ apps: string[], declaredBy: #SecretDeclarer[], set: bool, fingerprint?: string, setBy?: string, setAt?: uint }",
        )
        .of::<super::deployments::DeploymentSecretView>(),
        component(
            "InviteView",
            "An organization invite as its list shows it, never its token: `createdBy`/`createdAt` `null` for one made before they were kept.",
            "{ inviteKey: string, role: #OrgRole, teams: string[], usesLeft: uint, expiresAt: uint, expired: bool, createdBy: string | null, createdAt: uint | null, allApps: bool }",
        )
        .of::<super::orgs::InviteView>(),
        component(
            "MemberSecurity",
            "A member as the security listing shows them: their teams, how many passkeys (a number below 3, else `\"3+\"`), whether they hold a recovery code, the last UTC day they were active (`null`: never seen) and their live sessions.",
            "{ account: string, name: string | null, role: #OrgRole, teams: string[], passkeys: uint | \"3+\", recoveryCode: bool, lastActiveDay: string | null, sessions: uint }",
        )
        .of::<super::orgs::MemberSecurity>(),
        component(
            "Offer",
            "An app a deployment offers: the release new instances run, who offered it and when (Unix seconds).",
            "{ package: string, version: string, addedBy: string, addedAt: uint }",
        )
        .of::<crate::system::Offer>(),
        component(
            "PublicDeploymentApp",
            "An app a deployment offers, and the release its new instances run.",
            "{ app: string, package: string, version: string }",
        )
        .of::<super::deployments::PublicDeploymentApp>(),
        component(
            "ScopedTeam",
            "One package's app team after a scope: whether it was made, the instances moved onto it, the rows added, how it is joined now (`null`: not changed) and how many lost it by a close.",
            "{ package: string, teamId: string, created: bool, apps: string[], added: uint, join: #TeamJoin | null, dropped: uint }",
        )
        .of::<crate::system::ScopedTeam>(),
        component(
            "SecretDeclarer",
            "A release that declares a secret.",
            "{ package: string, version: string }",
        )
        .of::<super::deployments::SecretDeclarer>(),
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
            "`details` of a `412 CONFIRM_REQUIRED`: the WebAuthn challenge to sign under `rpId` with one of the passkeys `allowCredentials` names (credential ids; empty: any of the account's), and the `challengeId` the retry's `confirm` names. `apps`: deleting a team that holds apps, the apps that go with it. `then`: making an API key with follow-ups, the calls the passkey approves, `key:<keyId>` written in.",
            "{ challengeId: string, challenge: b64url, rpId: string, allowCredentials: string[], apps?: string[], then?: { app: string, method: string, args: any }[] }",
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
        )
        .of::<super::SessionAnswer>(),
        component(
            "BackupKeyAnswer",
            "A holder's backup key: who holds it (`{ kind: \"operator\" }`, `{ kind: \"org\", id }` or `{ kind: \"account\", id }`), the X25519 public key the recovery disk seals their apps' backups to (`null` while none is set), and how many apps that covers.",
            "{ holder: { kind: \"org\", id: string } | { kind: \"account\", id: string } | { kind: \"operator\" }, key: { publicKey: string, setAt: uint, setBy: string } | null, apps: uint }",
        )
        .of::<super::backup_keys::BackupKeyAnswer>(),
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
            "FollowUpFailed",
            "`details` of a `422 FOLLOW_UP_FAILED`: the API key's follow-up that failed (`index` in `then`, its app and method) and the error it answered, the follow-ups before it, which applied (`applied`, their indexes), and the failed one again in `unknown` when its answer (a 5xx, after it was sent again under its own key) could not say whether it applied; no key was made.",
            "{ index: uint, app: string, method: string, error: { status: uint, error: string, message: string, details?: any }, applied: uint[], unknown: uint[] }",
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
            "`details` of a `400 UNSUPPORTED_BUNDLE`, `400 BAD_BUNDLE`, `400 COMPUTE_ABI_MISMATCH` or `422 COMPILE_TIMEOUT` refusing one service of a release: the service, and `compute: true` when it was that service's compute component. Absent when the refusal is of a raw bundle.",
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
        )
        .of::<calimero_client::attest::Receipt>(),
        component(
            "Grant",
            "One way into an instance; an instance's `grants` are a list of them, and none is its owner alone. `org`: every member of its organization (not beside a team grant); `team`: members of that team; `account`: one person, a member of the organization (a named member) or not (a guest, admitted only while the instance's release says `guests: allowed` and its organization's `allowGuests` is on), with an optional `scope` the app defines and reads for its caller (opaque JSON, at most 256 bytes; the platform admits, the app scopes); `anyone`: any signed-in account, a member or not; never a caller without an account, who comes in only through a public link; `link`: a public link, whose token's holders call the instance's public methods as anonymous visitors (its key, never a token; made and revoked on `/apps/{app}/public-links` only, shown to whoever may change the instance's access, and kept as they are by `POST /apps/{app}/access`). At most 16 teams and 1000 accounts.",
            r#"{ kind: "org" } | { kind: "team", team: string } | { kind: "account", account: string, scope?: any } | { kind: "anyone" } | { kind: "link", link: string, label?: string, expiresAt?: uint, ratePerMinute: uint, createdBy: string, createdAt: uint }"#,
        )
        .of::<crate::types::Grant>(),
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
        )
        .of::<calimero_log::Status>(),
        component(
            "RollbackStatus",
            "A node's rollback counter: the cluster's epoch (rollbacks forced so far), its generation and mark, whether this start was forced (`--accept-rollback`), and the seconds since mero-kms last acknowledged a checkpoint.",
            "{ epoch: uint, generation: uint, mark: uint, forced: bool, sinceCheckpointSecs: uint }",
        )
        .of::<crate::rollback::Status>(),
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
            "UpgradePolicy",
            "How an organization's owner and admins let an app's updates apply (`POST /orgs/{org}/upgrade-policy`, `POST /apps/{app}/upgrade-policy`); every field is answered; a request may leave any out (its default). `secondApprover` (default off): a red update needs two distinct owners or admins. `autoApply` (default `never`; `noDestinations`: a green update that removes no method and moves none of the apps it created, of an app whose unit has no destinations before or after it, so no data leaves Calimero) and `holdHours` (24 to 168, default 24): such an update applies on its own once `holdHours` have passed since the release was logged (its install, or its deployment's offer), after a restore point.",
            r#"{ autoApply: "never" | "noDestinations", holdHours: uint, secondApprover: bool }"#,
        )
        .of::<crate::types::UpgradePolicy>(),
        component(
            "PolicyView",
            "An update policy at one scope: the organization's (`app` absent) or an app's. `policy` is the one in force; `source` where it comes from (the app's own, its organization's, or the defaults); `own` the one set at this scope, `null` for none.",
            r#"{ org: string, app?: string, policy: #UpgradePolicy, source: "app" | "org" | "default", own: #UpgradePolicy | null }"#,
        )
        .of::<crate::system::PolicyView>(),
        component(
            "PendingApproval",
            "A first approval of a red update waiting for its second (`POST /apps/{app}/upgrade-approvals`): the app approved (`app`), its target, a unit's members' (`targets`, by app), who approved and when, when it lapses (`expiresAt`, a week on), and the class its approver was shown.",
            r#"{ app: string, package: string, version: string, targets: { [string]: { package: string, version: string } }, by: string, at: uint, expiresAt: uint, class: "green" | "red", reasons: #UpdateReason[] }"#,
        )
        .of::<super::upgrades::PendingApprovalView>(),
        component(
            "UnitMember",
            "An app of a unit, by its path from the instance: its app id (absent on a release's card, where no app exists), the release it runs or would run, that release's signer, and the destinations it alone may send data to.",
            "{ app?: string, package: string, version: string, signer: string, destinations: string[] }",
        )
        .of::<crate::egress::UnitMember>(),
        component(
            "UpdateReason",
            "Why an update is red (`calimero_bundle::update::Reason`): `member` names a member by its path, `null` for the instance itself. `secretBindingChanged`: the release declares secret `name` it did not (`before` null) or binds it elsewhere (another host or place, or a path its earlier binding did not cover; fewer paths is not a change), each binding in one line. `unclassified`: the release a part runs or moves to (`package`, `version`) is no longer installed, so the update could not be classified and is taken as red.",
            r#"{ code: "signerChanged", member: string | null, from: string, to: string } | { code: "egressAdded", destinations: string[] } | { code: "stateVersionChanged", member: string | null, from: uint | null, to: uint | null } | { code: "memberAdded", member: string, package: string, egress: string[] } | { code: "memberRemoved", member: string, package: string, egress: string[] } | { code: "delegatedMethodsAdded", member: string | null, package: string, methods: string[] } | { code: "secretBindingChanged", member: string | null, name: string, before: string | null, after: string } | { code: "unclassified", member: string | null, package: string, version: string }"#,
        )
        .of::<calimero_bundle::update::Reason>(),
        component(
            "RestorePoint",
            "An app's one undo (`POST /apps/{app}/undo`): the release and bundle its last upgrade moved it from (`fromRelease`, `fromBundle`) and to, when (`at`, the start of the week to undo it) and by whom (`policy:auto-update` for an update that applied on its own), the run it was part of (a unit's members undo with their instance), the app group's snapshot (`file`: the node that holds it, absent where the upgrade ran nothing on the group) and `undoneAt` while an undo's restore is under way.",
            r#"{ fromBundle: string, fromCompute?: string, fromRelease?: #PackageRef, toBundle: string, toRelease?: #PackageRef, at: uint, by: string, run?: string, file?: { node: uint, group: uint, source: { name: string, sha256: string, index: uint } }, undoneAt?: uint }"#,
        )
        .of::<crate::types::RestorePoint>(),
        component(
            "UndoPart",
            "One app an undo reverts: the release it leaves (`from`) and returns to (`to`, `toBundle`), when it was upgraded and by whom (`auto`: on its own), until when it may be undone, its restore point, and what the undo loses (`lostWrites`: the app group's entries since the restore point, its last write, and `any`: whether it wrote since the upgrade, read from a majority of the replicas the app is on now, the furthest counting, never from the restore point's node alone; each of them `null` when nobody can say, never `false` for writes it cannot see; `lostWrites` itself `null` while the restore point's node does not answer, which `unavailable` then says).",
            r#"{ app: string, from: #PackageRef | null, to: #PackageRef | null, toBundle: string, upgradedAt: uint, upgradedBy: string, auto: bool, undoUntil: uint, restorePoint: { node: uint, index: uint, sha256: string } | null, undoneAt: uint | null, lostWrites: { entriesSince: uint | null, lastWriteAt: uint | null, any: bool | null } | null, unavailable: { code: string, message: string } | null }"#,
        )
        .of::<super::undo::UndoPart>(),
        component(
            "OutboundAuditEntry",
            "One outbound request of an instance: `atMs` (Unix milliseconds, the node's clock), `method`, `host` and `path` (no query string, at most 128 bytes, `pathTruncated` when cut; both empty for a URL that did not parse), the host's `status` or, absent that, the `failure` that stopped it (`malformed`, `refused` by a gate, `notApproved`, `awaitingApproval`, `egressOff` (egress turned off), `secret` (a credential binding refused, or a `{{secret:` placeholder), `failed` in transit), the names of the credential bindings the node attached in `secrets` (for a `secret` refusal, the binding refused), and the request's and response's body sizes. Never a secret value, a body, a query string or a header.",
            r#"{ atMs: uint, method: string, host: string, path: string, pathTruncated: bool, status?: uint, failure?: "malformed" | "refused" | "notApproved" | "awaitingApproval" | "egressOff" | "secret" | "failed", secrets: string[], requestBytes: uint, responseBytes: uint }"#,
        )
        .of::<calimero_outbound::AuditEntry>(),
        component(
            "OutboundLog",
            "An instance's outbound requests, newest first (`entries`), merged from every replica; `partial`: a replica did not answer, so its runs' requests may be missing.",
            "{ entries: #OutboundAuditEntry[], partial: bool }",
        )
        .of::<crate::node::OutboundLog>(),
        component(
            "UsageDay",
            "What an app's runs used on one UTC day (`date`, `YYYY-MM-DD`), each figure a decimal string; `wallMs` is information only.",
            "{ date: string, runs: u64, fuel: u64, wallMs: u64, writes: u64, bytesWritten: u64, events: u64 }",
        )
        .of::<super::apps::UsageDay>(),
        component(
            "RunFile",
            "A file a run reads or wrote: its id, and its plaintext size and SHA-256 (hex) only for a caller who may list the app's files.",
            "{ id: string, size?: uint, sha256?: string }",
        )
        .of::<crate::node::RunFileView>(),
        component(
            "Run",
            "A compute run's record, without its arguments or keys: its method, state, caps (`fuel` a JSON number here), inputs, attempts (`tsMs` Unix milliseconds) and final outcome (`null` while it runs; `message` the failure text, `null` unless the caller may list the app's files). `progress` is the leader's last poll (`null` elsewhere, when it is not active, or to a caller who may not list files); `queue` where the run waits for a job node, to a caller who may list files only; `logs` (deprecated: a run keeps no log) is always empty, to the same callers.",
            r#"{ run: string, method: string, state: "queued" | "running" | "done" | "failed" | "cancelled", cancelRequested: bool, startTsMs: uint, caps: { fuel: uint, memoryMib: uint, wallSecs: uint }, inputs: #RunFile[], attempts: { attempt: uint, node: uint, tsMs: uint }[], final: { attempt: uint, outcome: { kind: "done" } | { kind: "failed", reason: "fuel" | "memory" | "trap" | "app-error" | "integrity" | "cancelled" | "lost" | "refused" | "timeout" | "quota" | "not-granted" | "too-large" } | { kind: "cancelled" }, outputs: #RunFile[], usage: { cpuMicros: uint, gibMicros: uint, fuel: uint }, tsMs: uint, message: string | null } | null, progress: { percent: uint, step: string } | null, queue?: { state: "unplaced" | "waiting" | "granted", capacity: uint, pool: string, reason?: "NO_CAPACITY" | "POOL_FULL" | "PAYER_AT_RUN_QUOTA" | "PAYER_AT_FUEL_QUOTA" }, logs?: any[] }"#,
        )
        .of::<crate::node::RunView>(),
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
        )
        .of::<crate::origins::OriginView>(),
        component(
            "SignUpState",
            "The sign-up policy and what it implies: whether organization invites and cluster sign-up invites admit a new account, and whether one is required.",
            "{ policy: #SignUp, orgInvites: bool, signUpInvites: bool, inviteRequired: bool }",
        )
        .of::<super::sign_up::SignUpState>(),
        component(
            "ApprovedReleases",
            "The mero-tee releases the scope approves now, after the change.",
            "{ approvedReleases: string[] }",
        )
        .of::<crate::system::ApprovedReleases>(),
        component(
            "OutboundApproved",
            "The outbound approval the scope holds now for the package's publisher (`signer`), after the change.",
            "{ signer: string, approval: #OutboundApproval }",
        )
        .of::<crate::system::OutboundApproved>(),
        component(
            "Uninstalled",
            "The versions an uninstall removed.",
            "{ removed: string[] }",
        )
        .of::<crate::system::Uninstalled>(),
        component(
            "Deadlines",
            "Every release deadline after the change, oldest release first (`after` RFC 3339, `afterSecs` Unix seconds).",
            "{ deadlines: #VersionDeadline[] }",
        )
        .of::<crate::system::Deadlines>(),
        component(
            "SignUpSet",
            "The cluster's governance and sign-up policy after the change.",
            "{ governance: #Governance, signUp: #SignUpState }",
        )
        .of::<super::sign_up::SignUpSet>(),
        component(
            "InviteKey",
            "The key of the cluster sign-up invite withdrawn.",
            "{ inviteKey: string }",
        )
        .of::<crate::system::InviteKeyAnswer>(),
        component(
            "UpgradeRun",
            "An organization-wide upgrade run: the apps it targets, those done, and why each failed one failed.",
            "{ runId: string, package: string, version: string, team?: string, cascade: bool, by: string, startedAt: uint, apps: string[], done: string[], failed: { [string]: any }, finished: bool, targets?: { [string]: { package: string, version: string } } }",
        )
        .of::<crate::types::UpgradeRun>(),
        component(
            "App",
            "An app instance (a context) as the directory records it. `executor` is absent for an SDK 0.12 app (wasmer), which this node lists but does not run; `release` is the package release it runs, absent for a record from before or a raw upload; `restore` is a restore from a backup under way; `restorePoint` the restore point of its last upgrade, which an owner or admin may undo for a week (`POST /apps/{app}/undo`); `egressOff`: the instance's own egress switch is off (`POST /apps/{app}/egress`), absent while on.",
            r#"{ appId: string, name: string, group: uint, bundle: string, owner: string, org: string | null, grants: #Grant[], confirmMethods: string[], executor?: "component" | "wasmer", replicas: uint[], createdAt: uint, initArgs: any, computeBundle?: string, components?: "computeOnly", metadata: any, lastUpgrade?: #Upgrade, deployment?: string, release?: #PackageRef, restore?: { seed?: uint, source?: { name: string, sha256: string, index: uint } }, links?: { [string]: string }, attached?: { [string]: string }, restorePoint?: #RestorePoint, egressOff?: bool }"#,
        )
        .of::<crate::types::AppRecord>(),
        component(
            "AppEntry",
            "An app as `GET /apps` lists it: `package` and `version` name the release it runs where the caller may read it (`null` otherwise, and for a raw upload); `?package=` keeps the instances whose row names that package, `deployment` the deployment it was created from (`null` for none; absent from an older node, as 1.0.0-rc.1).",
            "{ appId: string, name: string, owner: string, org: string, bundle: string, package: string | null, version: string | null, deployment: string | null }",
        )
        .of::<super::apps::AppEntry>(),
        component(
            "PackageRef",
            "One service of one package release: `service` is absent for a single-service bundle, `compute` (absent: false) marks the service's compute component.",
            "{ package: string, version: string, service?: string, compute?: bool }",
        )
        .of::<crate::system::PackageRef>(),
        component(
            "Upgrade",
            "The last upgrade tried on an app, done or failed: `from` and `to` are bundle hashes, `by` the account (or `operator`), `run` the organization-wide run it was part of, `apiKey` the upgrade key that made it. `class` and `reasons`: what the update classifier said of it, judged again where it applied (absent before cluster version 1.0.0-rc.34, for a raw bundle and a failed upgrade); `approvedBy`: the first approver of a red update whose policy asks for a second. `auto`: it applied on its own under the app's update policy (`by` is `policy:auto-update`); `undo`: it undid the upgrade before it (`fromRelease` the release undone).",
            r#"{ from: string, to: string, at: uint, by: string, ok: bool, error?: any, run?: string, apiKey?: string, fromRelease?: #PackageRef, toRelease?: #PackageRef, class?: "green" | "red", reasons?: #UpdateReason[], approvedBy?: string, auto?: bool, undo?: bool }"#,
        )
        .of::<crate::types::UpgradeRecord>(),
        component(
            "Package",
            "A package and the releases of it the caller may see, oldest first; `latest` is the newest of those. A release's `visibility`: `public` (everyone here uses it; a release the operator installed, the cluster's catalog, is always), `private` (members of the organizations it was installed for; none: the accounts that installed it), absent for one recorded before the node kept it. `orgs`: of a private release's organizations, only those the caller is a member of.",
            r#"{ package: string, signerId: string, owner: string, latest: string, versions: { version: string, mpk: string, size: uint, source: "registry" | "upload", services: { [string]: string }, components: "app" | "computeOnly", metadata: any, outbound: string[], installedBy: string, installedAt: uint, visibility?: "public" | "private", orgs?: { orgId: string, name: string }[] }[] }"#,
        )
        .of::<super::packages::PackageAnswer>(),
        component(
            "ApiKey",
            "An API key as the account's list shows it (never its secret): `apps` and `methods` are `null` for every one; `upgrades` makes it an upgrade key.",
            "{ keyId: string, account: string, label: string, apps: string[] | null, methods: string[] | null, readOnly: bool, allowConfirm: bool, expiresAt: uint, upgrades?: { app: string, package: string, signer: string, proofKey: string } }",
        )
        .of::<crate::types::ApiKeyRecord>(),
        component(
            "Org",
            "An organization as `GET /orgs/{org}` answers it. `members` maps each account to its role, `capabilities` and `defaultCapabilities` are capability bits, `outboundApprovals` is keyed by package signer, `upgradeRuns` by run id; `recovery` is absent when the organization set no recovery key, `allApps` when no member gets every app added later, `personal` unless it is a personal organization a sign-up made before cluster version 1.0.0-rc.33 (from that version it is an organization like any other). `allowGuests`: whether its instances take guests (its owner or an admin sets it, `POST /orgs/{org}/settings`; absent from a node before 1.0.0-rc.33). `egressOff`: its egress switch is off, so none of its apps sends anything out of the cluster (`POST /orgs/{org}/egress`). `upgradePolicy`, `appUpgradePolicies` and `pendingUpgrades`: its update policy, its apps' own and the first approvals of red updates waiting for a second (absent when none).",
            r#"{ orgId: string, name: string, members: { [string]: #OrgRole }, teams: { [string]: #Team }, defaultCapabilities: uint, capabilities: { [string]: uint }, approvedReleases: string[], outboundApprovals: { [string]: #OutboundApproval }, metadata: any, memberMetadata: { [string]: any }, upgradeRuns: { [string]: #UpgradeRun }, recovery?: { publicKey: string, setAt: uint, pending: { account: string, startedAt: uint, readyAt: uint } | null }, allApps?: string[], personal?: bool, allowGuests?: bool, allowPublicLinks?: bool, egressOff?: bool, upgradePolicy?: #UpgradePolicy, appUpgradePolicies?: { [string]: #UpgradePolicy }, pendingUpgrades?: { [string]: { package: string, version: string, targets?: { [string]: { package: string, version: string } }, by: string, at: uint, class: { class: "green" | "red", reasons?: #UpdateReason[] } } }, mode?: "legacy" | "standalone" | "shared", releaseApprovers?: string[], publishes?: string[], newWorkStopped?: { release: string, after: string } | null }"#,
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
            "A team as `GET /orgs/{org}/teams/{team}` answers it: each member's row, the child teams the caller sees, its apps, and the caller's own row (`myRole`, `null` for none) and membership (`member`). `package` names the package of an app team (`null` for any other), `defaultCapabilities` the capabilities a new member gets (`null`: the organization's).",
            "{ teamId: string, name: string, parent: string | null, join: #TeamJoin, metadata: any, package: string | null, defaultCapabilities: uint | null, members: { account: string, role: #TeamRole, capabilities: uint }[], children: string[], apps: { appId: string, name: string, contextId: string }[], myRole: #TeamRole | null, member: bool }",
        )
        .of::<super::orgs::TeamView>(),
        component(
            "Deployment",
            "A deployment as its managers list it. `host` is its frontend host, `null` on a node with no apps domain; times are RFC 3339.",
            "{ id: string, host: string | null, name: string, owner: #Deployer, apps: #DeploymentOffer[], createdBy: string, createdAt: string }",        )
        .of::<super::deployments::DeploymentView>(),
        component(
            "DeploymentShown",
            "The deployment an instance comes from, as its customers are shown it: names are not unique, so always with its id and its owner's kind and name (`null` once the owner is gone).",
            r#"{ id: string, name: string, owner: { kind: "account" | "org", name: string | null } }"#,
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
            "The scope whose approvals cover an app: its organization (`personal: true` when it is its owner's personal organization, below cluster version 1.0.0-rc.32 only). `account` is only `GET /auth/apps`' own scope: the apps the account owns, wherever they are.",
            r#"{ kind: "org", org: string, personal?: bool } | { kind: "account" }"#,
        )
        .of::<super::AppScope>(),
        component(
            "ClusterRelease",
            "A mero-tee release the cluster's nodes run: how many run it, and whether the scope approved it (`true` where nothing was ever recorded).",
            "{ release: string, nodes: uint, approved: bool }",
        )
        .of::<crate::report::ClusterRelease>(),
        component(
            "BundleRelease",
            "A bundle by its digest, with the release it came from where the reader may see it.",
            "{ bundle: string, package?: string, version?: string }",
        ),
        component(
            "ReportUpgrade",
            "An app's last upgrade as a report shows it. `actor`, `error`, `run` and `apiKey` are shown to the app's people, never to the operator.",
            r#"{ from: #BundleRelease, to: #BundleRelease, at: uint, ok: bool, actorKind: "owner" | "member" | "operator", class?: "green" | "red", actor?: string, error?: any, run?: string, apiKey?: string, reasons?: #UpdateReason[], approvedBy?: string }"#,
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
        )
        .of::<super::sign_up::SignUpInviteView>(),
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
    /// Always answered (`org_get`), though the state leaves `true` out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allow_guests: Option<bool>,
    /// As `allow_guests`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allow_public_links: Option<bool>,
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
    const EGRESS: &str = "{ egress: bool, confirm?: #Confirm }";
    vec![
        // ---------------------------------------------------------------- public
        get("/health", "health", Anyone, "Node health")
            .about("In the clear. The node's id, the system group's leader (`null`: none known), its `merod` release, the newest cluster version it supports (`clusterVersion`) and the cluster's rollback epoch (`null` on a node without a rollback counter).")
            .ok("#Health"),
        get("/attestation", "attestation", Anyone, "The node's attestation")
            .about("In the clear. A TDX quote binding the node's transport key (or, with `purpose=receipts`, its receipt key and the governance; with `purpose=kms`, what it verified of its KMS; with `purpose=orchestrator`, the cluster identity this replica holds) to `nonce`; `app`/`appHash` bind an app's bundle hash: `app` only an app anyone may reach (public), any other answering `404` as an unknown id does, so it never says that a private app exists. A client checks it before it seals anything to the key. The node makes one quote at a time and lets a few requests wait for it: past that `503 BUSY` with `Retry-After: 1`, ask again then.")
            .query::<super::AttestQuery>(r#"{ nonce: string, app?: string, appHash?: string, purpose?: "receipts" | "kms" | "orchestrator" }"#)
            .ok("#AttestationDoc")
            .errors(&[(400, "BAD_NONCE"), (400, "BAD_HASH"), (404, "NOT_FOUND"), (503, "BUSY")]),
        get("/releases", "releases_public", Anyone, "The cluster's releases")
            .about("The cluster's governance and the deadlines set on mero-tee releases, oldest release first: after `after` (RFC 3339; `afterSecs` Unix seconds) a scope that has not approved that release (or a newer one) starts no new work.")
            .ok("#ClusterReleases"),
        get("/publishers/{signer}", "publisher_binding", SignedIn, "Whether a signing key is bound")
            .about("Whether a package signing key (`did:key`) is bound to an organization on this cluster (`bound`, never which), and its binding generation (`0`: never bound).")
            .ok("#PublisherBinding"),
        get("/sign-up", "sign_up_public", Anyone, "The sign-up policy")
            .about("Who may create an account here, the cluster's name and the tag a sign-in uses to pick the person's passkey for it.")
            .ok("#SignUpPublic"),
        // ---------------------------------------------------------------- accounts
        post("/auth/passkey/register/begin", "register_begin", Anyone, "Start a sign-up")
            .about("Creation options for a new passkey. `invite` is an organization or cluster sign-up invite token, which the policy may require; `403 SIGN_UP_REFUSED` when the policy would refuse the sign-up. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their shape is WebAuthn's.")
            .json::<super::RegisterBegin>("{ name?: string, origin?: string, invite?: string }")
            .ok("#PasskeyChallenge")
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
            .ok("#PasskeyChallenge")
            .errors(&[(429, "RATE_LIMITED")]),
        post("/auth/passkey/login/finish", "login_finish", Anyone, "Finish a sign-in")
            .json::<super::AssertionBody>("{ challengeId: string, assertion: #Assertion }")
            .ok("#Session")
            .errors(&[(400, "WRONG_CHALLENGE"), (400, "UNKNOWN_CHALLENGE"), (400, "EXPIRED_CHALLENGE"), (401, "PASSKEY_UNKNOWN_HERE"), (421, "WRONG_NODE")]),
        post("/auth/passkeys/add/begin", "passkey_add_begin", AccountHolder, "Start adding a passkey")
            .about("Creation options for another passkey (a device) on the caller's account. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their shape is WebAuthn's.")
            .ok("#PasskeyChallenge"),
        post("/auth/passkeys/add/finish", "passkey_add_finish", AccountHolder, "Finish adding a passkey")
            .about("Answers `412` first; sent again with `confirm` signed by a passkey the account already has.")
            .json::<super::AddPasskeyFinish>("{ challengeId: string, registration: #Registration, confirm?: #Confirm }")
            .ok("#PasskeyAdded")
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
        get("/auth/backup-key", "account_backup_key", AccountHolder, "The account's backup key")
            .about("The X25519 public key the recovery-disk backups of the account's personal apps are sealed to (`key`, `null` while none is set).")
            .ok("#BackupKeyAnswer"),
        post("/auth/backup-key", "account_set_backup_key", AccountHolder, "Set the account's backup key")
            .about("`publicKey`: 64 lowercase hex, an X25519 public key whose private half the account holder keeps (never a node), for its personal apps, where owners upgrade apps. `412` first.")
            .json::<super::BackupKeyBody>("#BackupKeyBody")
            .ok("#BackupKeyAnswer")
            .step_up()
            .errors(&[(400, "BAD_BACKUP_KEY"), (403, "FORBIDDEN"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/auth/recovery", "recovery_create", AccountHolder, "Make a recovery code")
            .about("Shown once, replacing any; `412` first.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("#RecoveryCode")
            .step_up()
            .errors(&[(400, "WRONG_CHALLENGE")]),
        delete("/auth/recovery", "recovery_remove", AccountHolder, "Remove the recovery code")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up(),
        post("/auth/recovery/begin", "account_recovery_begin", Anyone, "Start an account recovery")
            .about("Signed out, with the recovery code: creation options for the passkey that will replace the lost ones. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their shape is WebAuthn's.")
            .json::<super::RecoveryBegin>("{ code: string }")
            .ok("#PasskeyChallenge")
            .errors(&[(421, "WRONG_NODE"), (429, "RATE_LIMITED")]),
        post("/auth/recovery/start", "account_recovery_start", Anyone, "Spend the recovery code")
            .about("Logs the new passkey as pending; it may finish the recovery once `readyAt` passed (72 h). `401` for a wrong or spent code.")
            .json::<super::RecoveryStart>("{ challengeId: string, code: string, registration: #Registration }")
            .ok("#AccountRecoveryPending")
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
            .ok("#SessionsRevoked"),
        get("/auth/sessions", "sessions_list", AccountHolder, "The account's live sessions")
            .about("Latest active first, at most 256. `openedAt` (Unix seconds) is `null` for a session opened before the node kept it.")
            .ok("#SessionList"),
        delete("/auth/sessions/{session}", "session_revoke", AccountHolder, "End one session")
            .about("`404` for a session the account does not hold live.")
            .ok("{}")
            .errors(&[(404, "NOT_FOUND")]),
        get("/auth/activity", "activity_list", AccountHolder, "What the account's passkeys did")
            .about("Newest first, the last 100: sign-ins (`app`, `origin`) and approvals (`method`, `subject`); `after` keeps the entries after that Unix second.")
            .query::<super::ActivityQuery>("{ after?: uint }")
            .ok("#ActivityList"),
        get("/auth/me", "me", SignedIn, "The caller's account")
            .about("The account, its passkeys (with `addedAt` and `lastSignInAt`), organizations (`personal`: one a sign-up made before cluster version 1.0.0-rc.32, an organization like any other from that version; a sign-up makes none from then on), recovery state, and how the caller came (`via`).")
            .ok("#AccountView")
            .errors(&[(404, "NOT_FOUND")]),
        post("/auth/account-proof", "account_proof::prove", Session, "Prove the account to another party")
            .about("A statement signed with this node's receipt key that the caller's passkey session asked to bind `nonce` for `audience`.")
            .json::<super::account_proof::ProofBody>("{ nonce: string, audience: string }")
            .ok("#AccountProof")
            .errors(&[(400, "BAD_AUDIENCE"), (400, "BAD_NONCE")]),
        get("/auth/apps", "account_apps", SignedIn, "The account's own apps")
            .about("The apps the account owns, in any organization (a package-bound session: its package's), with its only organization's `approvedReleases` (below cluster version 1.0.0-rc.32 its personal organization's): each app's release, the newer releases it may upgrade to, its last upgrade, its replicas' releases and `lastWriteAt`.")
            .query::<super::PageQuery>(PAGE)
            .ok("#AppsReport"),
        get("/auth/usage", "usage", SignedIn, "The account's usage against its quotas")
            .about("The apps the account owns, its bundle bytes (a decimal string), API keys and owned organizations; the file bytes held by each payer it belongs to (each organization it is in, and `account:<id>` for its own apps); the cluster's quotas; and what today's runs of the apps the account develops used (decimal strings).")
            .ok("#AccountUsage"),
        get("/auth/aliases", "alias_list", SignedIn, "The account's aliases").ok("#AliasList"),
        get("/auth/aliases/{alias}", "alias_get", SignedIn, "Resolve an alias")
            .ok("#AliasView")
            .errors(&[(404, "NOT_FOUND")]),
        put("/auth/aliases/{alias}", "alias_set", SignedIn, "Set an alias")
            .json::<super::AliasBody>("{ app: string }")
            .ok("{}")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/auth/aliases/{alias}", "alias_delete", SignedIn, "Remove an alias").ok("{}"),
        post("/auth/apikeys", "apikey_create", AccountHolder, "Make an API key")
            .about("Scoped to `apps` and `methods`, read-only or not, for `ttlSecs`; `upgrades` makes an upgrade key, which signs upgrades of one app in place of a passkey; only an owner or admin of the app's organization makes one (`403 FORBIDDEN`), and its upgrades are judged by that rule again where they apply. `then` (at most 4, 64 KiB as JSON; not with `upgrades`) names confirm call methods of the key's own `apps` that the same passkey approves, each naming the new key: every string argument exactly `key:$key` is written `key:<keyId>` (a follow-up without one is `400 BAD_FOLLOW_UP`), the consent and the system log state each call's arguments only by hash (`argsHash`: SHA-256 of a 16-byte salt, then the arguments as the `412` echoes them, keys sorted by UTF-8 bytes, in compact JSON), the `412` details list them so (`then`), and the retry runs them in order before making the key; the first that fails answers `422 FOLLOW_UP_FAILED` (`details: { index, app, method, error, applied }`) and no key is made. A key write that fails after follow-ups ran adds `applied` and `keyWrite` (`refused`, or `unknown` when it may still land: list the keys and revoke it if unwanted) to its error's `details`. A node before 1.0.0-rc.36 ignores `then` and makes a plain key: a client checks that the `412` lists `then` and that the answer's `then` is as long as its own. `412` first. The key is shown once.")
            .json::<super::ApiKeyBody>("{ label?: string, apps?: string[], methods?: string[], readOnly?: bool, allowConfirm?: bool, ttlSecs?: uint, upgrades?: { app: string, package?: string }, then?: { app: string, method: string, args?: any }[], confirm?: #Confirm }")
            .ok("#ApiKeyMade")
            .step_up()
            .errors(&[(400, "BAD_FOLLOW_UP"), (400, "BAD_UPGRADE_KEY"), (400, "RESERVED_METHOD"), (400, "WRONG_CHALLENGE"), (409, "FEATURE_NOT_ACTIVE"), (413, "ARGS_TOO_LARGE"), (422, "FOLLOW_UP_FAILED"), (429, "QUOTA_EXCEEDED")]),
        get("/auth/apikeys", "apikey_list", AccountHolder, "The account's API keys").ok("#ApiKeyList"),
        delete("/auth/apikeys/{key}", "apikey_revoke", AccountHolder, "Revoke an API key")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        // ---------------------------------------------------------------- packages
        post("/bundles", "bundle_put", Installer, "Upload a raw component")
            .about("A `calimero:app` component, 16 MiB at most; only on a node started with `--allow-unsigned-bundles` or `--dev`. A WASM core module (SDK 0.12) is refused `400 UNSUPPORTED_BUNDLE`.")
            .raw("application/wasm", "the component")
            .ok("#BundleStored")
            .errors(&[(400, "BAD_BUNDLE"), (400, "UNSUPPORTED_BUNDLE"), (403, "FORBIDDEN"), (413, "TOO_LARGE"), (422, "COMPILE_TIMEOUT"), (429, "QUOTA_EXCEEDED"), (503, "BUSY")]),
        post("/packages", "package_upload", Installer, "Upload a signed release")
            .about("A signed `.mpk`, checked as an install from the registry is (signature, artifact digests, `minRuntimeVersion`, the package's pinned signer, every service compiles). Only with `--dev`; otherwise `403 UPLOADS_DISABLED`. An upload is private to the organization `org` names (the caller a member, else `404`), else the caller's only one: `400 ORG_REQUIRED` in several; in none, it is private to the accounts that install it.")
            .query::<super::UploadQuery>("{ org?: string }")
            .raw("application/octet-stream", "the `.mpk`")
            .ok("#PackageInstalled")
            .errors(&[(400, "BAD_PACKAGE"), (400, "BAD_BUNDLE"), (400, "COMPUTE_ABI_MISMATCH"), (400, "UNSUPPORTED_BUNDLE"), (400, "ORG_REQUIRED"), (403, "UPLOADS_DISABLED"), (403, "DEPLOY_NOT_GRANTED"), (404, "NOT_FOUND"), (413, "TOO_LARGE"), (422, "COMPILE_TIMEOUT"), (503, "BUSY")]),
        get("/packages", "package_list", SignedIn, "Installed packages")
            .about("Only the releases the caller may see.")
            .ok("#PackageList"),
        post("/packages/install", "package_install", Installer, "Install a release from the registry")
            .about("For the organization `org` names (the caller a member, else `404`), else the caller's only one (`400 ORG_REQUIRED` in several). A release the registry serves without its credential is public: everyone here uses it. Else it is fetched with the credential and the caller's account, and is private: only the members of the organizations it was installed for use it, and an install for another adds that one; installed for none (the caller is in none), only the accounts that install it use it. `404` when the registry serves it to neither; `403 ACCOUNT_NOT_LINKED` when the registry refuses the caller's account as linked to no Calimero Cloud developer (link it in Calimero Cloud, under node accounts).")
            .json::<super::AccountInstallBody>("{ package: string, version: string, org?: string }")
            .ok("#PackageInstalled")
            .errors(&[(400, "BAD_PACKAGE"), (400, "BAD_BUNDLE"), (400, "COMPUTE_ABI_MISMATCH"), (400, "UNSUPPORTED_BUNDLE"), (400, "NO_REGISTRY"), (400, "ORG_REQUIRED"), (403, "ACCOUNT_NOT_LINKED"), (403, "DEPLOY_NOT_GRANTED"), (404, "NOT_FOUND"), (413, "TOO_LARGE"), (422, "COMPILE_TIMEOUT"), (503, "BUSY")]),
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
            .ok("#OriginList"),
        post("/package-origins/{package}", "origins::add", AccountHolder, "Register a frontend origin")
            .about("By whoever manages the package, with a passkey step-up over the change. Answers the origin as registered (the one already there, when the same origin is registered again).")
            .json::<super::origins::AddOriginBody>("{ origin: string, rpId?: string, served?: bool, confirm?: #Confirm }")
            .ok("#PackageOrigin")
            .step_up()
            .errors(&[(400, "BAD_RP_ID"), (429, "QUOTA_EXCEEDED")]),
        delete("/package-origins/{package}", "origins::remove", AccountHolder, "Remove a frontend origin")
            .json::<super::origins::RemoveOriginBody>("{ origin: string, confirm?: #Confirm }")
            .ok("#OriginRemoved")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        // ---------------------------------------------------------------- apps
        post("/apps", "app_create", Installer, "Create an app instance")
            .about("From a release (`package`, `version`, `service`), a deployment's offer (`deployment`), or an uploaded `bundle`. Every app is in an organization, chosen by the release's signed `instanceOrg` (a raw bundle: `either`): `standalone` makes a new organization with the instance (its name `newOrg.name`, else the instance's), and naming `org` is `400 STANDALONE_INSTANCE`; `org` goes into `org`, else the caller's only organization (`400 ORG_REQUIRED` with several, `409 NO_ORGANIZATION` with none; `newOrg` is `400 ORG_INSTANCE`); `either` goes into `org`, a new one when `newOrg` asks, else the caller's only one, else (none) a new one, `400 ORG_REQUIRED` with several. Below cluster version 1.0.0-rc.32 an instance naming none goes to the caller's personal organization, and `newOrg` is `409 FEATURE_NOT_ACTIVE`. `grants` say who reaches it (#Grant; absent: the organization; empty: the owner alone; any grant needs cluster version 1.0.0-rc.33, `409 FEATURE_NOT_ACTIVE`; the single `access` mode of earlier releases is `400 ACCESS_REMOVED`, never read as no grants). `initArgs` go to the app's `init` and `metadata` is kept as given (both free-form JSON). Answers once `init` committed; `compute` says whether the app has a compute component.")
            .bytes_json::<super::CreateAppBody>("{ name?: string, bundle?: string, package?: string, version?: string, service?: string, org?: string, newOrg?: { name: string }, grants?: #Grant[], initArgs?: any, metadata?: any, deployment?: string }")
            .ok("#AppCreated")
            .errors(&[(400, "BAD_REQUEST"), (400, "ACCESS_REMOVED"), (400, "NOT_OFFERED"), (400, "VERSION_REQUIRED"), (400, "NO_SUCH_SERVICE"), (400, "AMBIGUOUS_COMPUTE"), (400, "ORG_REQUIRED"), (400, "STANDALONE_INSTANCE"), (400, "ORG_INSTANCE"), (403, "ACCOUNT_NOT_LINKED"), (409, "NO_ORGANIZATION"), (409, "FEATURE_NOT_ACTIVE"), (409, "RELEASE_DEADLINE"), (409, "RUNTIME_TOO_OLD"), (422, "INIT_FAILED"), (429, "QUOTA_EXCEEDED"), (503, "UNAVAILABLE")]),
        get("/apps", "app_list", SignedIn, "Apps the caller reaches")
            .about("The instances the caller reaches; one an `anyone` grant admits is listed only to its owner.")
            .query::<super::AppListQuery>("{ package?: string, bundle?: string }")
            .ok("#AppList"),
        get("/apps/{app}", "app_get", SignedIn, "One app")
            .about("`{app}` is an app id or its context id (`hex(SHA-256(app_id))`), here and on every `/apps/{app}` route. A guest sees only their own grant in `grants`.")
            .ok("#App")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/apps/{app}", "app_delete", Session, "Delete an app")
            .about("The owner or an organization admin. An instance whose release created apps with it (`uses` with `mode: create`, its `links`) is deleted with every one of them, theirs included, listed in `members`; a member is not deleted alone while its instance stands (`409 UNIT_MEMBER`), and an instance with members waits for every node to run 1.0.0-rc.32 (`409 FEATURE_NOT_ACTIVE`).")
            .ok("#AppDeleted")
            .errors(&[(409, "UNIT_MEMBER"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/apps/{app}/metadata", "app_metadata", SignedIn, "Set an app's metadata")
            .about("The owner or an organization admin; 16 KiB at most. `metadata` is the caller's own JSON, free-form, kept as given.")
            .json::<super::MetadataBody>("#MetadataBody")
            .ok("{}")
            .errors(&[(413, "TOO_LARGE")]),
        get("/apps/{app}/storage", "app_storage", AppAccess, "What an app stores")
            .about("Keys, bytes and blobs held, and `lastWriteAt` (Unix seconds of its last committed write, `null` before one).")
            .ok("#AppStorage"),
        get("/apps/{app}/usage", "app_usage", SignedIn, "An app's runs per day")
            .about("The owner or an organization admin: `today` and the 30 `days` before it, figures as decimal strings; `wallMs` is information only.")
            .ok("#AppUsage")
            .errors(&[(403, "FORBIDDEN")]),
        get("/apps/{app}/outbound-log", "app_outbound_log", SignedIn, "An app's outbound requests")
            .about("The owner or an organization admin: the outbound HTTPS requests the instance's runs made, sent or refused, newest first, at most 100 (`MAX_AUDIT_ENTRIES`) and none older than 7 days (`AUDIT_RETENTION_MS`). Each replica keeps those its own runs made, in memory, until it restarts; `partial` says a replica did not answer. An entry holds no secret value, body, query string or header.")
            .ok("#OutboundLog")
            .errors(&[(403, "FORBIDDEN")]),
        get("/apps/{app}/abi", "app_abi", AppAccess, "An app's ABI")
            .about("The ABI its bundle embeds: methods, events, errors and state. `abi` is the bundle's own document, free-form here: its format is the ABI's, not this API's.")
            .ok("#AppAbi"),
        get("/apps/{app}/openapi.json", "app_openapi", AppAccess, "An app's OpenAPI document")
            .about("Generated from the ABI of the bundle the app runs, under the access `/abi` has: one `POST /apps/{app}/methods/{method}` per method, with its arguments, result and errors. Free-form here: it is a document of its own, which a client reads as OpenAPI.")
            .ok("#AppOpenApi")
            .errors(&[(404, "NOT_FOUND"), (410, "UNSUPPORTED_ENGINE"), (422, "BAD_ABI")]),
        get("/apps/{app}/runs", "app_runs", AppAccess, "An app's compute runs")
            .about("Newest first, `limit` (1 to 100, default 20) at a time; `next` is the `before` of the next page, `null` on the last.")
            .query::<super::RunsQuery>("{ limit?: string, before?: string }")
            .ok("#RunList")
            .errors(&[(400, "BAD_CURSOR"), (400, "BAD_LIMIT"), (404, "NOT_FOUND")]),
        get("/apps/{app}/runs/{run}", "app_run", AppAccess, "One compute run")
            .about("Its record without arguments or keys: method, state, `cancelRequested`, `startTsMs`, attempts, input and output ids (sizes and SHA-256 only for callers who may list the app's files), usage, and `progress` as the leader last polled it.")
            .ok("#Run")
            .errors(&[(404, "NOT_FOUND")]),
        get("/apps/{app}/runs/{run}/logs", "app_run_logs", SignedIn, "A compute run's attempt logs")
            .about("Deprecated: always `{ logs: [] }`. A run keeps no log, as an app's log lines (stdout, stderr, `log`) may hold user data, which neither Calimero nor the operator may see; a log a release up to 1.0.0-rc.35 kept is answered to no one, and the files export does not know it. Only for those who may handle the app's files. To read a compute method's log lines, run it locally (`meroctl local compute run`).")
            .ok("#RunLogs")
            .errors(&[(404, "NOT_FOUND")]),
        get("/apps/{app}/presence", "presence_get", AppAccess, "Who is present")
            .about("One entry per present device (a session's passkey, or an API key): its account, device and state (hex).")
            .ok("#PresenceList"),
        post("/apps/{app}/presence", "presence_publish", AppAccess, "Publish presence")
            .about("The caller's presence state (hex), in sequence; not logged. A `null` (or absent) `state` retracts it.")
            .json::<super::PresenceBody>("{ seq: uint, state?: string | null }")
            .ok("{}")
            .errors(&[(400, "BAD_STATE"), (413, "TOO_LARGE")]),
        get("/apps/{app}/presence/stream", "presence_stream", AppAccess, "Presence as server-sent events")
            .events("a `snapshot`, then `presence` diffs"),
        post("/apps/{app}/intents", "intents", AppAccess, "Call a method")
            .about("A logged call: the result, the events of the call the caller receives, and the answering node's receipt. A `confirm` method answers `412` first. An app error is `422 APP_ERROR` with `details { index, code, fields }`; a trap `422 TRAP`. `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them. An `idempotencyKey` starting `node:apikey:` is the node's own (an API key's follow-ups): `400 RESERVED_IDEMPOTENCY_KEY`.")
            .json::<super::IntentBody>("{ method: string, args?: any, idempotencyKey?: string, confirm?: #Confirm }")
            .ok("#IntentResult")
            .errors(&[(400, "RESERVED_IDEMPOTENCY_KEY"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "RELEASE_DEADLINE"), (412, "CONFIRM_REQUIRED"), (413, "ARGS_TOO_LARGE"), (422, "APP_ERROR"), (422, "TRAP"), (429, "QUOTA_EXCEEDED"), (503, "TIMEOUT"), (503, "UNAVAILABLE")])
            .step_up_when("the method is a `confirm` method of the app's ABI")
            .retry_safe_with("idempotencyKey"),
        get("/apps/{app}/attestation", "app_attestation", AppAccess, "Every replica's receipt-key attestation")
            .about("Each replica's `/attestation?purpose=receipts` for `nonce` (32 bytes, hex), or why it could not be had (`error`), and `cluster`, the cluster id each quote binds (with the replica's `node`): the documents are fetched in the clear, so their own `cluster` and `node` fields count for nothing. Any other query parameter is ignored.")
            .query::<super::AppAttestQuery>("{ nonce: string }")
            .ok("#AppAttestations"),
        post("/apps/{app}/views", "views", AppAccess, "Read with a view method")
            .about("Read linearizably (`consistency: \"strong\"`, the default; any value but `\"stale\"` reads as it): the replica answering waits until it applied the index the app's leader names once it confirmed it leads, and sends the view to the leader if it cannot catch up; or (`\"stale\"`) on the replica answering, as it stands; `minIndex` waits until the replica applied it (`503 NOT_CAUGHT_UP`). `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them.")
            .json::<super::ViewBody>(r#"{ method: string, args?: any, consistency?: "strong" | "stale", minIndex?: uint }"#)
            .ok("#ViewResult")
            .errors(&[(422, "APP_ERROR"), (422, "TRAP"), (503, "NOT_CAUGHT_UP"), (503, "TIMEOUT")])
            .retry_safe(),
        post("/apps/{app}/methods/{method}", "app_method", AppAccess, "Call a method by its path")
            .about("The body of `/intents` or `/views` without `method`, which the path names, as the app's OpenAPI document lists it: a view the running bundle's ABI declares (without `confirm`) is answered as `/views`, anything else as `/intents`, with that route's gate, step-up and limits. Retry-safe as a view always, as a call with `idempotencyKey`. `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them.")
            .json::<super::MethodBody>(r#"{ args?: any, idempotencyKey?: string, confirm?: #Confirm, consistency?: "strong" | "stale", minIndex?: uint }"#)
            .ok("#MethodResult")
            .errors(&[(400, "RESERVED_IDEMPOTENCY_KEY"), (400, "RESERVED_METHOD"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "RELEASE_DEADLINE"), (412, "CONFIRM_REQUIRED"), (413, "ARGS_TOO_LARGE"), (422, "APP_ERROR"), (422, "TRAP"), (429, "QUOTA_EXCEEDED"), (503, "NOT_CAUGHT_UP"), (503, "TIMEOUT"), (503, "UNAVAILABLE")])
            .step_up_when("the method is a `confirm` method of the app's ABI")
            .retry_safe_with("idempotencyKey"),
        get("/apps/{app}/events", "events", AppAccess, "An app's events as server-sent events")
            .about("Committed events from index `from`, then live ones, each to its audience.")
            .query::<super::EventsQuery>("{ from?: uint }")
            .events("each committed event the caller is in the audience of")
            .errors(&[(429, "RATE_LIMITED"), (503, "UNAVAILABLE")]),
        post("/apps/{app}/upgrade", "app_upgrade", AccountHolder, "Upgrade an app instance")
            .about("To a release (`version`, of the app's own package unless `package` names another) or a `bundle`, code only: the state stays and is read by the new code. An owner or admin of the app's organization, whatever the package's manifest declares (`403 FORBIDDEN` for anyone else, before the step-up; on a cluster still below 1.0.0-rc.32, as before: whoever the manifest names), with a step-up over `{ bundle, migrate, manifest }`; an upgrade key signs in place of it, and its account is judged the same. A refused or failed upgrade leaves the app on its release and answers `422 UPGRADE_FAILED` with the app log's error in `details` (`MigrationsNotSupported` when `migrate` is named: there are no migrations in this version). `error` is the app log's error value (`null` when it ran), free-form: the app's own. An instance created from a deployment moves only to the release its deployment offers for its package (naming no version accepts that offer): another version, another package or a `bundle` is `409 OUTSIDE_DEPLOYMENT`, before the step-up. Where the app's policy asks for a second approver, an update the log cannot classify (a part's release no longer installed) is `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.35, before the step-up.")
            .json::<super::UpgradeBody>("{ bundle?: string, package?: string, version?: string, service?: string, confirm?: #Confirm, migrate?: string }")
            .ok("#UpgradeApplied")
            .step_up()
            .errors(&[(400, "VERSION_REQUIRED"), (400, "BAD_BUNDLE"), (400, "AMBIGUOUS_COMPUTE"), (403, "FORBIDDEN"), (409, "CONFLICT"), (409, "OUTSIDE_DEPLOYMENT"), (409, "OUTSIDE_UNIT_RANGE"), (409, "RELEASE_DEADLINE"), (409, "RUNTIME_TOO_OLD"), (409, "FEATURE_NOT_ACTIVE"), (422, "UPGRADE_FAILED")]),
        get("/apps/{app}/update", "app_update", AppAccess, "Whether an update is available for an app")
            .about("For whoever may read the app (`GET /apps/{app}`'s rule; a session bound to its package's frontend included), so its frontend can offer the update and open the consent page. `available` carries the update classifier's verdict (`calimero_bundle::update::classify`; absent for an instance on a development bundle), over the whole unit when the instance has members and the caller is a member of its organization or may use every release they move to, as `upgrade-unit` would move them, else the app alone: `class` `red` when the signer changes, egress grows, a state version changes, a member with egress joins or leaves, or a release newly calls another app as the person (`reasons` lists each), else `green`; `egress`: the unit's destinations `before` and `after` (empty: no data leaves Calimero; an external endpoint, a TEE included, is a destination like any other), with those `added` and `removed`; `newMethods` and `removedMethods` (a member's as `<path>.<method>`). `available`: for an instance from a deployment, the release the deployment offers for its package, only while that offer serves (someone acting for the deployment's owner may use it), when it is not the one running (`source: deployment`); for any other, the newest release of its package the caller sees, newer than the running one, that the upgrade would take (`source: release`, as a report's `latestReachable`); else `null`. `unit`: whether the instance has apps of its own (`links`), which move with it: review it with `upgradeUnit`. `mayApprove`: whether the caller may approve the upgrade. `approvers`: `owner-and-admins` (the owner and admins of the app's organization), `operator` where the cluster gives it app upgrades, or `managers` on a cluster still below 1.0.0-rc.32 (the rule before: the owner or `MANAGE_APPLICATION` holders, or the publisher's). `policy`: the update policy the app follows and its `source`. `available.summary`: the update in one plain line for a banner (the new destinations it adds, else \"Nothing changes where your data goes\" when green, else its first reason), the same words the consent page and the node's updates page use; the design's `GET /apps/{app}/updates` is this route. `available.approvalsNeeded`: 2 for a red (or unclassified) update where the policy asks for a second approver, else 1; `available.approvedBy`: who approved it first, while that approval waits. `pendingApprovals`: the first approvals waiting for their second that cover this app (its own, or its unit's). `deployment`: the deployment the instance comes from, which chooses its updates, else `null`.")
            .ok("#UpdateView")
            .errors(&[(404, "NOT_FOUND")]),
        get("/apps/{app}/egress", "app_egress", AppAccess, "Where an app's unit may send data")
            .about("For whoever may read the app (`GET /apps/{app}`'s rule). `destinations`: every destination (`outbound` prefix, and a declared secret's host no prefix names) of the release the instance runs and of each app it created with it (`links`, transitively; never an attached app), sorted; empty when no data leaves Calimero. An external endpoint, a TEE included, is listed like any other: the platform never vouches for one (an app that trusts one verifies it in its own code). `members`: each member by its path (`<name>`, a member's own `<name>/<name>`) with its app id, release, signer and own destinations. A development bundle declares none. `egress`: `off` while its organization (`offBy: organization`) or the instance (`instance`) turned egress off (`offBy` and `summary` are absent while it is on), when none of the destinations is reached and `summary` says the app cannot send data outside the cluster.")
            .ok("#EgressView")
            .errors(&[(404, "NOT_FOUND")]),
        get("/apps/{app}/upgrade-policy", "app_policy", AppAccess, "The update policy an app follows")
            .about("For whoever may read the app. Its own policy where an owner or admin set one, else its organization's, else the defaults: nothing updates on its own, one approval applies an update.")
            .ok("#PolicyView")
            .errors(&[(404, "NOT_FOUND")]),
        post("/apps/{app}/upgrade-policy", "app_policy_set", AccountHolder, "Set an app's own update policy")
            .about("An owner or admin of the app's organization, with a step-up over `org:<org> / __set_upgrade_policy / { app, policy }`. `policy: null` (or left out) drops it: the app follows its organization's again. `holdHours` outside 24 to 168 is `400 BAD_UPGRADE_POLICY` before the step-up. A move to another organization drops it. `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.")
            .json::<super::UpgradePolicyBody>("#UpgradePolicyBody")
            .ok("#PolicyView")
            .step_up()
            .errors(&[(400, "BAD_UPGRADE_POLICY"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/apps/{app}/upgrade-approvals", "app_approve_upgrade", AccountHolder, "Approve a red update first, for a second approver to apply")
            .about("Where the app's policy asks for a second approver (`409 ONE_APPROVAL` otherwise, and for a green update), a red update needs two distinct owners or admins of its organization: this logs the first approval, its own consent over `<app> / __approve_upgrade / { package, version, targets, class }` (the class the classifier gives now, over the unit with `unit`). Another owner or admin then applies it with `POST /apps/{app}/upgrade` (or `/upgrade-unit` for a unit's approval), which until then answers `409 SECOND_APPROVAL_REQUIRED`. One waits per app, for a week at most; a newer one replaces it, and another approver's of the same update is `409 APPROVAL_PENDING` (apply it instead). `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34, and for an update the log cannot classify (a part's release no longer installed: class `red` with `unclassified` reasons) below 1.0.0-rc.35, before the step-up.")
            .json::<super::UpgradeApprovalBody>("{ version: string, unit?: bool, confirm?: #Confirm }")
            .ok("#ApprovalLogged")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "ONE_APPROVAL"), (409, "APPROVAL_PENDING"), (409, "NOT_INSTALLED"), (409, "OUTSIDE_DEPLOYMENT"), (409, "FEATURE_NOT_ACTIVE"), (409, "CONFLICT")]),
        get("/apps/{app}/undo", "app_undo_view", AppAccess, "The upgrade an app may undo")
            .about("For whoever may read the app. `available`: whether an owner or admin may undo its last upgrade now (within a week of it, while it runs what the upgrade moved it to, its restore point's node answering); else `reason` says why. `apps`: the app, and the members its unit's upgrade moved with it, each with what the undo loses: the restore point is the state just before the upgrade, so whatever the app wrote since is gone.")
            .ok("#UndoView")
            .errors(&[(404, "NOT_FOUND")]),
        post("/apps/{app}/undo", "app_undo", AccountHolder, "Undo an app's last upgrade")
            .about("An owner or admin of the app's organization, within a week of the upgrade (judged on the logged times), with a step-up over `<app> / __undo_upgrade / { apps: { <app>: { to, bundle, from, restorePoint } } }`. The app, and the members its unit's upgrade moved, go back to the release they ran before and, where the upgrade ran on their group, to their restore point's state in a new group (seeded by the node that holds it, as a restore is): the one move to an older release. Whatever they wrote since the restore point is lost (`apps[].lostWrites`, as `GET` showed it). Answers once each group takes calls again, or after a minute with `restored: false` (calls answer `409 APP_RESTORING` meanwhile; sending the undo again seeds it again). `409 NO_RESTORE_POINT`, `UNDO_WINDOW_PASSED`, `UNDO_STALE` (the app moved on), `UNDO_RELEASE_GONE` (the release it would return to is no longer installed), `UNDO_NOT_AS_NAMED` (a member its unit's run moved: undo its instance), `RESTORE_POINT_UNAVAILABLE` (its node does not answer), each before the step-up; `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.")
            .json::<super::undo::UndoBody>("{ confirm?: #Confirm }")
            .ok("#UndoDone")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "NO_RESTORE_POINT"), (409, "UNDO_WINDOW_PASSED"), (409, "UNDO_STALE"), (409, "UNDO_RELEASE_GONE"), (409, "UNDO_NOT_AS_NAMED"), (409, "RESTORE_POINT_UNAVAILABLE"), (409, "FEATURE_NOT_ACTIVE"), (409, "UPGRADE_PENDING")]),
        post("/apps/{app}/upgrade-unit", "app_upgrade_unit", AccountHolder, "Upgrade an app and the apps it created, in one consent")
            .about("An instance created with apps of its own (`links`): it moves to `version` of its package, and each member the target release's `uses` gives a release other than the one it runs moves to that release (its lock's, else the newest installed one inside the range), all under one step-up over `{ app, package, version, targets }`. Answers the run (`GET /orgs/{org}/upgrades/{run}` follows it); each app is upgraded on its own as an organization's run does. `400 NOT_A_UNIT` for an app with no members; `409 CONFLICT` when the target release creates a member the instance lacks. A version the caller may not use answers as one that is not installed, as `POST /apps` installs one (`404 NOT_FOUND`, `400 NO_REGISTRY`), and for an instance from a deployment anything but its offer that still serves `409 OUTSIDE_DEPLOYMENT`, before anything of it is read.")
            .json::<super::upgrades::UnitUpgradeBody>("{ version: string, confirm?: #Confirm }")
            .ok("#UpgradeRun")
            .step_up()
            .errors(&[(400, "NOT_A_UNIT"), (400, "NO_REGISTRY"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "CONFLICT"), (409, "OUTSIDE_DEPLOYMENT"), (409, "FEATURE_NOT_ACTIVE"), (409, "USES_NOT_INSTALLED")]),
        get("/apps/{app}/secrets", "app_secrets", SignedIn, "The secrets an app's release declares")
            .about("For each secret the release the instance runs declares (its signed manifest's `secrets`): its binding, the one `host`, the path prefixes there it is attached under (`paths`, on whole segments; `/` for every path) and the one place the node attaches it (`header` with its `format`, or `bodyField`), with `binding` saying it in one line (\"sends OPENAI_API_KEY as the Authorization header (Bearer …) to api.openai.com under /v1/\"), its description and whether it is required, and `source`: `app` when the instance has its own value (with its `fingerprint`, the first 16 hex digits of its SHA-256, `setBy` and `setAt`), else `deployment` when the deployment it was created from holds one (`deployment_secret_set`; nothing more of it), else `cluster` (the operator's, if it bound one). Never a value. The owner, or whoever manages the organization's apps.")
            .ok("#AppSecrets")
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        put("/apps/{app}/secrets/{name}", "app_secret_set", AccountHolder, "Set an app's own value of a declared secret")
            .about("Attached by the node only where the secret's binding says (its host, header or body field), never shown again (`app_secrets` answers its fingerprint), kept in the app's log on its replicas, out of the app's own reach. The owner, or whoever manages the organization's apps, with a step-up over `{ name, fingerprint }`: the value is never in the consent. `404` for a name the release does not declare; 1 to 4,096 bytes (`400 BAD_SECRET`); `409 CLUSTER_NOT_UPGRADED` while a node runs an earlier release.")
            .json::<super::apps::SecretBody>("{ value: string, confirm?: #Confirm }")
            .ok("#SecretSet")
            .step_up()
            .errors(&[(400, "BAD_SECRET"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "CONFLICT"), (409, "CLUSTER_NOT_UPGRADED")]),
        delete("/apps/{app}/secrets/{name}", "app_secret_clear", AccountHolder, "Remove an app's own value of a secret")
            .about("The operator's value applies again. Same rules as setting one; a step-up over `{ name }`.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("#SecretCleared")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "CLUSTER_NOT_UPGRADED")]),
        post("/apps/{app}/access", "app_access", Session, "Set who may reach an app")
            .about("`grants` replace the instance's grant list (#Grant; empty: the owner alone; cluster version 1.0.0-rc.33, else `409 FEATURE_NOT_ACTIVE`). Its owner, or whoever manages the organization's apps (on each team it admits, before and after). A package-bound session: only its own package's app, from one of its package's teams to another.")
            .json::<super::AccessBody>("{ grants: #Grant[] }")
            .ok("{}"),
        post("/apps/{app}/invites", "app_invite_create", AccountHolder, "Invite someone to an instance")
            .about("A token whose redeemer is named on this instance only (an `account` grant: a guest, or a named member when they are a member of its organization; never added to the organization), `uses` times (1 to 1000, default 1) within `ttlSecs` (at most 90 days, default 7), on any instance. `scope` (opaque JSON, at most 256 bytes) goes on the grant it makes, for the app to read: the platform admits, the app scopes (one channel of a chat, say). Whoever may change the instance's access, whatever its guest switches say: they are judged when someone redeems it. At most 64 pending per instance. `412` first.")
            .json::<super::AppInviteBody>("{ ttlSecs?: uint, uses?: uint, scope?: any, confirm?: #Confirm }")
            .ok("#AppInviteMade")
            .step_up()
            .errors(&[(400, "BAD_REQUEST"), (400, "WRONG_CHALLENGE"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "FEATURE_NOT_ACTIVE"), (429, "QUOTA_EXCEEDED")]),
        get("/apps/{app}/invites", "app_invite_list", Session, "An instance's invites")
            .about("For whoever may change its access: keys, terms and scope, never a token.")
            .ok("#AppInviteList")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/apps/{app}/invites/{key}", "app_invite_revoke", AccountHolder, "Revoke an instance invite")
            .about("Its creator, or whoever may change the instance's access; `412` first.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        post("/apps/{app}/public-links", "public_link_create", AccountHolder, "Make a public link of an instance")
            .about("A link whose token lets anyone call the instance's `#[app::public]` methods as an anonymous visitor (`Caller::Visitor { link, key }`), whatever its access: send `Authorization: Bearer vis1.<token>.<visitor key>.<at>.<signature>`, the visitor's own Ed25519 key (64 hex) signing `\"calimero.visitor.v1\\0\" ‖ link ‖ 0 ‖ key ‖ 0 ‖ at` (hex) with `at` within 5 minutes of the node's clock. Whoever may change the instance's access, while its organization allows public links (`409 PUBLIC_LINKS_OFF`). `ratePerMinute` (1 to 600, default 60) bounds each visitor's calls on each node; `expiresAt` (Unix seconds) is optional. The token is answered once; the log keeps only its public key (`link`). At most 16 per instance. `412` first; `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.33.")
            .json::<super::PublicLinkBody>("{ label?: string, expiresAt?: uint, ratePerMinute?: uint, confirm?: #Confirm }")
            .ok("#PublicLinkMade")
            .step_up()
            .errors(&[(400, "BAD_REQUEST"), (400, "WRONG_CHALLENGE"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "PUBLIC_LINKS_OFF"), (409, "FEATURE_NOT_ACTIVE"), (429, "QUOTA_EXCEEDED")]),
        get("/apps/{app}/public-links", "public_link_list", Session, "An instance's public links")
            .about("For whoever may change its access: each link's key, terms and creator, never a token.")
            .ok("#PublicLinks")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/apps/{app}/public-links/{link}", "public_link_revoke", AccountHolder, "Revoke a public link")
            .about("Its creator, or whoever may change the instance's access; it admits nobody from the next request. `412` first.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        post("/apps/{app}/transfer", "app_transfer", AccountHolder, "Hand an app to another account")
            .about("The owner (for an organization's app, while a member and in the app's team), or an organization admin; only to a member of the app's organization (`400 REJECTED` before the passkey is asked for; below cluster version 1.0.0-rc.32 an app in a personal organization went to the new owner's personal one). Answers `null`.")
            .json::<super::TransferBody>("{ to: string, confirm?: #Confirm }")
            .ok("null")
            .step_up(),
        get("/apps/{app}/delegations", "app_delegations", SignedIn, "What you let an app do as you")
            .about("The apps you let this app call as you, each with the methods (`SetDelegation`). A session bound to an app's frontend reads its own app's only.")
            .ok("#Delegations"),
        put("/apps/{app}/delegations/{target}", "app_delegate", AccountHolder, "Let an app call another as you")
            .about("The methods of `target` that the release `app` runs lists for `target`'s package in its signed `uses` (`methods`), with your passkey over exactly those. You must reach both apps, which are in one organization. From then on, when `app` calls one of those methods while serving you, `target` runs it with you as the caller and reads `env::via()` = `app`; without the grant it runs as `app`, and `target` never learns who `app` was serving. A method that needs your passkey confirmation is still refused there. `409 NOTHING_TO_DELEGATE` when the release lists none. A session opened on an app's frontend is refused (`403`): the app asks on the consent page instead (`POST /consent/requests { kind: \"delegate\" }`).")
            .json::<super::apps::DelegateBody>("{ confirm?: #Confirm }")
            .ok("#DelegationMade")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "NOTHING_TO_DELEGATE"), (409, "FEATURE_NOT_ACTIVE")]),
        delete("/apps/{app}/delegations/{target}", "app_undelegate", SignedIn, "Take back what an app may do on another as you")
            .about("No passkey: it only narrows. `404 NOT_DELEGATED` when you granted nothing there.")
            .ok("{}")
            .errors(&[(404, "NOT_DELEGATED"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/apps/{app}/move", "app_move", AccountHolder, "Move an app to another organization")
            .about("Its data where it is; its files, runs and quotas go to the target, its `org` or app-team grant becomes the target's app team of its package. `bringMembers` adds the source's members who reach it, capped at `member`. `from` is the organization it left, `null` for an app of the caller's own; `added` the accounts brought; `revokedInvites` how many invites to its old team were withdrawn.")
            .json::<super::MoveBody>("{ toOrg: string, bringMembers?: bool, confirm?: #Confirm }")
            .ok("#AppMoved")
            .step_up()
            .errors(&[(409, "FEATURE_NOT_ACTIVE")]),
        get("/apps/{app}/links", "app_links", AppAccess, "List an app's links to the apps its release uses")
            .about("One row per reference (`uses`) of the release the app runs, with the app linked under it: a member made with the instance (`mode: create`) or an app its owner attached (`mode: attach`), `null` when none. `version` is the release that app runs and `inRange` whether it is inside the reference's range (`null` with nothing linked). A link the release no longer names is listed with `package`, `range` and `mode` `null`.")
            .ok("#AppLinkList"),
        put("/apps/{app}/links/{name}", "app_link", AccountHolder, "Attach an app to one of an app's references")
            .about("`name` is an `attach` reference of the release the app runs, and `app` an app of the same organization that runs its package; the caller manages both. One outside the reference's range is attached anyway and answered `inRange: false`, and the consent page warns about it on later upgrades: an attached app is never upgraded with the instance. `env::linked(name)` answers it from the next run on. Answers the link as `GET /apps/{app}/links` lists it.")
            .json::<super::apps::LinkBody>("{ app: string, confirm?: #Confirm }")
            .ok("#LinkRow")
            .step_up()
            .errors(&[(400, "NOT_ATTACHABLE"), (403, "FORBIDDEN"), (409, "NOT_AN_ATTACH_REFERENCE"), (409, "WRONG_PACKAGE"), (409, "FEATURE_NOT_ACTIVE")]),
        delete("/apps/{app}/links/{name}", "app_unlink", AccountHolder, "Detach an app attached to an app")
            .about("Whoever manages the app. A member made with the instance is never detached.")
            .json::<super::apps::UnlinkBody>("{ confirm?: #Confirm }")
            .ok("{}")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_ATTACHED"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/apps/{app}/blobs", "blob_upload", AppAccess, "Upload a blob")
            .about("A plain blob, stored with the request's `Content-Type`, which a component app's download answers with: any media type (printable ASCII, 127 bytes at most; another is `400 BAD_REQUEST`), `application/octet-stream` when the request names none. Send the blob's own type, not `*/*`, which would be stored as its type. Any `sealed` parameter, `true` or `false`, is refused `400 USE_FILES_API`: sealed files go through `/files`.")
            .query::<super::BlobUploadQuery>("{ sealed?: bool }")
            .raw("*/*", "the blob; its `Content-Type` is the type it is stored and served with")
            .ok("#BlobStored")
            .errors(&[(400, "USE_FILES_API"), (413, "TOO_LARGE"), (429, "QUOTA_EXCEEDED")]),
        get("/apps/{app}/blobs", "blob_list", AppAccess, "List an app's blobs")
            .about("Plain blobs only.")
            .ok("#BlobList"),
        get("/apps/{app}/blobs/{id}", "blob_download", AppAccess, "Download a blob")
            .about("`{id}` is base58 or hex; `grant` is a link the app made (`BlobRef::grant`).")
            .query::<super::BlobGrant>("{ grant?: string }")
            .any_bytes("the blob: a component app's under the MIME type it was stored with, otherwise `application/octet-stream`")
            .errors(&[(404, "NOT_FOUND"), (503, "UNAVAILABLE")]),
        delete("/apps/{app}/blobs/{id}", "blob_delete", AppAccess, "Forget a blob")
            .about("The owner or an organization admin.")
            .ok("#Logged")
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        get("/apps/{app}/blobs/{id}/info", "blob_info", AppAccess, "A blob's size")
            .ok("#BlobView")
            .errors(&[(404, "NOT_FOUND"), (409, "NOT_SUPPORTED")]),
        post("/apps/{app}/blobs/{id}/revoke", "blob_revoke", AppAccess, "Invalidate a blob's grants")
            .about("The grants made so far for a published blob; the owner or an organization admin.")
            .ok("#Logged")
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        post("/apps/{app}/files", "files_upload", SignedIn, "Upload a sealed file")
            .about("Up to 16 MiB in one request; the owner and the organization's members.")
            .raw("application/octet-stream", "the file")
            .ok("#FileStored")
            .errors(&[(403, "FORBIDDEN"), (413, "TOO_LARGE"), (429, "QUOTA_EXCEEDED")]),
        get("/apps/{app}/files", "files_list", SignedIn, "List an app's sealed files")
            .ok("#FileList"),
        post("/apps/{app}/files/uploads", "upload_begin", SignedIn, "Begin a file upload")
            .about("A session for a file of `size` bytes, sent in parts of `partBytes`.")
            .json::<super::BeginUpload>("{ size: uint }")
            .ok("#UploadView")
            .errors(&[(413, "TOO_LARGE"), (429, "QUOTA_EXCEEDED")]),
        get("/apps/{app}/files/uploads/{upload}", "upload_status", SignedIn, "Where an upload is")
            .ok("#UploadStatus"),
        delete("/apps/{app}/files/uploads/{upload}", "upload_abort", SignedIn, "Abort an upload")
            .ok("{}"),
        put("/apps/{app}/files/uploads/{upload}/parts/{k}", "upload_part", SignedIn, "Send one part")
            .about("Part `k` (from 0), raw bytes; a part out of order (`409 PART_OUT_OF_ORDER`), or one that differs from the part sent before as `k` (`409 PART_CONFLICT`), answers the next one expected in `details.next`.")
            .raw("application/octet-stream", "the part")
            .ok("#PartTaken")
            .errors(&[(400, "BAD_REQUEST"), (400, "BAD_PART"), (409, "PART_CONFLICT"), (409, "PART_OUT_OF_ORDER"), (503, "FILES_UNAVAILABLE")]),
        post("/apps/{app}/files/uploads/{upload}/complete", "upload_complete", SignedIn, "Commit an upload")
            .about("Charges and commits the file; a retry answers the same file. `409 UPLOAD_INCOMPLETE` names the part still missing.")
            .ok("#FileStored")
            .errors(&[(409, "UPLOAD_INCOMPLETE")]),
        get("/apps/{app}/files/{id}", "file_info", SignedIn, "One sealed file")
            .about("Its plaintext size and SHA-256, never its key.")
            .ok("#FileView")
            .errors(&[(404, "NOT_FOUND")]),
        post("/apps/{app}/files/{id}/export", "exports::export", SignedIn, "Export a sealed file's key")
            .about("The owner and the organization's members. `recipient` is an X25519 public key (64 hex characters); the file's data key comes back wrapped to it, with the file's `segments` (each one's hash, hex) and `ephemeral` public key. The node never answers plaintext or a key in the clear. 30 a minute per account.")
            .json::<super::exports::ExportBody>("{ recipient: string }")
            .ok("#FileExport")
            .errors(&[(400, "BAD_RECIPIENT"), (400, "BAD_REQUEST"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (429, "TOO_MANY_EXPORTS")]),
        get("/apps/{app}/files/{id}/segments/{k}", "exports::segment", SignedIn, "One segment of a sealed file")
            .about("Segment `k` (from 0) of the file's ciphertext, raw; the owner and the organization's members.")
            .any_bytes("the segment's ciphertext, `application/octet-stream`")
            .errors(&[(400, "BAD_REQUEST"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (404, "NO_SUCH_SEGMENT")]),
        post("/pipelines/validate", "pipelines::validate", SignedIn, "Validate a pipeline")
            .about("Every problem of `source`; with `package` and `version` (both, a release the caller sees) also against that release's methods, modules and the cluster's pools. Writes nothing.")
            .json::<super::pipelines::ValidateBody>("{ source: string, package?: string, version?: string }")
            .ok("#PipelineValidation")
            .errors(&[(400, "BAD_RELEASE"), (404, "NOT_FOUND")]),
        get("/packages/{package}/{version}/pipelines", "package_pipelines", SignedIn, "A release's pipelines")
            .about("The pipelines the release registers, by name: `hash`, `worstCaseFuel` (a decimal string), the `modules` they pin and the canonical `pipeline`. `404` for a release the caller may not see.")
            .ok("#ReleasePipelines")
            .errors(&[(404, "NOT_FOUND")]),
        get("/packages/{package}/{version}/capabilities", "package_capabilities", SignedIn, "A release's capabilities")
            .about("What the release is and may do, read from its signed manifest and the ABI its service component embeds, before any app of it exists: `bundle` is the service WASM's hash, `outbound` the sorted prefixes it may call out to and `abi` the embedded `calimero:abi` verbatim (`null` when it embeds none). `destinations` is where an instance of it could send data, over the release and every app its `uses` would create with it (`members`, by path, each with its release, signer and own destinations), empty when no data leaves Calimero; both `null` while a release its `uses` would create is not installed here. `service` names the service whose card to read (default: the release's only one). A session bound to the package reads what its account does (every release of it recorded before releases were public or private). `404` for a release the caller may not see, `400 NO_SUCH_SERVICE` for an unknown service or no `service` on a release with several, `400 BAD_REQUEST` for an over-long `service`.")
            .query::<super::packages::CapabilitiesQuery>("{ service?: string }")
            .ok("#CapabilityCard")
            .errors(&[(400, "BAD_REQUEST"), (400, "NO_SUCH_SERVICE"), (404, "NOT_FOUND")]),
        post("/apps/{app}/pipelines/{name}/estimate", "pipelines::estimate_run", SignedIn, "Estimate a pipeline run")
            .about("The owner and the organization's members. Runs, fuel and cost of a run of a pipeline the app's release registers, for `inputs` (file ids, only counted) or `counts`, not both; numbers as decimal strings.")
            .json::<super::pipelines::EstimateBody>("{ inputs?: { [string]: string[] }, counts?: { [string]: uint }, params?: { [string]: any } }")
            .ok("#RunEstimate")
            .errors(&[(400, "BAD_INPUTS"), (400, "BAD_PARAMS"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (404, "NO_SUCH_PIPELINE")]),
        post("/apps/{app}/pipelines/{name}/runs", "pipelines::start", SignedIn, "Start a pipeline run")
            .about("The owner and the organization's members. Starts a run of a pipeline the app's release registers. `maxCost` (micro-units, a decimal string) caps the run, and `worstCase` answers it.")
            .json::<super::pipelines::StartBody>("{ inputs?: { [string]: string[] }, params?: { [string]: any }, maxCost?: string }")
            .ok("#PipelineStarted")
            .errors(&[(400, "BAD_INPUTS"), (400, "BAD_MAX_COST"), (400, "BAD_PARAMS"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (404, "NO_SUCH_PIPELINE"), (409, "CLUSTER_NOT_UPGRADED"), (409, "NO_PRICE_TABLE"), (410, "OUTPUT_EXPIRED"), (422, "PIPELINES_NEED_ONE_SERVICE"), (429, "TOO_MANY_PIPELINE_RUNS"), (503, "UNAVAILABLE")]),
        get("/apps/{app}/pipeline-runs", "pipelines::list", AppAccess, "An app's pipeline runs")
            .about("The recent runs, newest first, as `{ runs }`; `limit` 1 to 100. Sizes, hashes and results only for callers who may list the app's files.")
            .query::<super::pipelines::ListQuery>("{ limit?: string }")
            .ok("#PipelineRunList")
            .errors(&[(400, "BAD_LIMIT"), (404, "NOT_FOUND")]),
        get("/apps/{app}/pipeline-runs/{run}", "pipelines::get", AppAccess, "One pipeline run")
            .about("Its record: `state`, `version`, `hash`, `computeBundle`, `inputs`, `params`, per-step `status`, `runs`, `outputs`, `result`, `reason` and `cost` (`cost` and `costSoFar` for a run with a cap). Sizes, hashes and results only for callers who may list the app's files.")
            .ok("#PipelineRunView")
            .errors(&[(404, "NOT_FOUND")]),
        get("/apps/{app}/pipeline-runs/{run}/events", "pipeline_events::events", AppAccess, "A pipeline run as server-sent events")
            .about("Whoever may read the run. Metadata only; at most 64 streams per app per node, and one lasts an hour.")
            .events("a `snapshot`, then `run` and `step` per change, then `end`, and the stream closes")
            .errors(&[(404, "NOT_FOUND"), (429, "TOO_MANY_SUBSCRIBERS")]),
        get("/apps/{app}/pipeline-runs/{run}/receipt", "pipeline_receipt::receipt", SignedIn, "A pipeline run's receipt")
            .about("The owner and the organization's members. For an ended run: the receipt, the answering node and its signature over it, by the node's receipt key. `version=2` adds what each job's report signs to its proof (`job`) and each job node's attestation (`jobAttestations`); the default, `1`, leaves them out.")
            .query::<super::pipeline_receipt::ReceiptQuery>("{ version?: uint }")
            .ok("#SignedPipelineReceipt")
            .errors(&[(400, "BAD_VERSION"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "RUN_NOT_ENDED")]),
        post("/apps/{app}/pipeline-runs/{run}/cancel", "pipelines::cancel", SignedIn, "Cancel a pipeline run")
            .about("The owner and the organization's members. Its pending steps end cancelled and its running steps' runs stop.")
            .ok("#PipelineCancelled")
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        post("/apps/{app}/pipeline-runs/{run}/retry", "pipelines::retry", SignedIn, "Retry a pipeline run")
            .about("The owner and the organization's members. A new run of an ended run's pipeline at the same release, hash and compute component, with the same inputs and params, the `limits` applied and `retryOf` set.")
            .json::<super::pipelines::RetryBody>("{ limits?: { [string]: { fuel?: uint, memoryMib?: uint, wallSecs?: uint } }, maxCost?: string }")
            .ok("#PipelineStarted")
            .errors(&[(400, "BAD_LIMITS"), (400, "BAD_MAX_COST"), (400, "LIMIT_EXCEEDS_CAP"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "CLUSTER_NOT_UPGRADED"), (409, "NO_PRICE_TABLE"), (409, "STILL_RUNNING"), (410, "OUTPUT_EXPIRED"), (429, "TOO_MANY_PIPELINE_RUNS"), (503, "UNAVAILABLE")]),
        get("/apps/{app}/xcalls", "app_xcalls", SignedIn, "An app's cross-app calls")
            .about("The owner or an organization admin: how many calls wait in its outbox for delivery (`pending`, a count), and its dead letters, oldest first (`atMs`: the entry time it was dead-lettered at, Unix milliseconds).")
            .ok("#XcallsReport")
            .errors(&[(403, "FORBIDDEN"), (503, "UNAVAILABLE")]),
        // ---------------------------------------------------------------- organizations
        post("/orgs", "org_create", Session, "Create an organization")
            .about("`recoveryKey` is the public half of an organization recovery key. `capabilities` (bits) is what members can do unless set for them, in place of the default; `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.29. From cluster version 1.0.0-rc.32 a sign-up makes no organization: an account's are those it creates here (or with a `standalone` instance) or is invited to; before, every account also had a personal organization from sign-up (`personal` in `/auth/me`).")
            .json::<super::OrgBody>("#OrgBody")
            .ok("#OrgCreated")
            .errors(&[(400, "BAD_REQUEST"), (400, "UNKNOWN_CAPABILITIES"), (409, "FEATURE_NOT_ACTIVE"), (429, "QUOTA_EXCEEDED")]),
        get("/orgs/{org}", "org_get", SignedIn, "One organization")
            .about("As the directory records it, with only the teams the caller reaches or manages and the people they see; and its `mode` (from its app teams), the accounts that may approve its releases (`releaseApprovers`), the keys it publishes under (`publishes`), and `newWorkStopped`: the release whose passed deadline stops new work in it (`after`, RFC 3339), `null` when none does. Those four are absent from a node older than they are (1.0.0-rc.1 sends none of them).")
            .ok("#Org")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/orgs/{org}", "org_delete", AccountHolder, "Delete an organization")
            .about("With its teams, invites and apps; the owner. `apps`: the apps deleted with it.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("#OrgDeleted")
            .step_up(),
        post("/orgs/{org}/metadata", "org_metadata", SignedIn, "Set an organization's metadata")
            .about("`metadata` is the caller's own JSON, free-form, kept as given.")
            .json::<super::MetadataBody>("#MetadataBody")
            .ok("{}"),
        get("/orgs/{org}/usage", "org_usage", SignedIn, "An organization's usage")
            .about("An admin: how many apps the organization develops, what their runs used today (decimal strings) and the cluster's quotas; a member gets `403`.")
            .ok("#OrgUsage")
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        post("/orgs/{org}/upgrade", "org_upgrade", AccountHolder, "Start an upgrade run")
            .about("Upgrade every app of the organization (or of `team`, with `cascade` its subtree) that runs `package` to `version`, with one passkey assertion over the whole run; the organization's owner or an admin (on a cluster still below 1.0.0-rc.32, whoever holds `MANAGE_APPLICATION` there). Each app is upgraded as `POST /apps/{app}/upgrade` would; `GET /orgs/{org}/upgrades/{run}` follows it.")
            .json::<super::OrgUpgradeBody>("#OrgUpgradeBody")
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
            .json::<super::MetadataBody>("#MetadataBody")
            .ok("{}"),
        get("/orgs/{org}/members", "org_members", AccountHolder, "Members' lockout risk")
            .about("`detail=security` (the only listing: without it, or with another value, `400 BAD_REQUEST`), the owner or an admin: each member's passkeys (`1`, `2` or `\"3+\"`), recovery code, last active UTC day and sessions. `403` below admin, `404` outside the organization.")
            .query::<super::MembersQuery>(r#"{ detail?: "security" }"#)
            .ok("#MemberSecurityList")
            .errors(&[(400, "BAD_REQUEST"), (403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        post("/orgs/{org}/members", "org_set_member", Session, "Set a member's role")
            .about("`owner` moves ownership. `412` first.")
            .json::<super::MemberBody>("#MemberBody")
            .ok("{}")
            .step_up(),
        delete("/orgs/{org}/members/{account}", "org_remove_member", Session, "Remove a member, or leave")
            .about("`412` first; the retry's JSON body is `{ confirm }`.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up(),
        post("/orgs/{org}/members/{account}/teams", "member_set_teams", Session, "Set a member's rows in several teams")
            .about("Each team judged as `POST /orgs/{org}/teams/{team}/members` would judge it, and none set unless every one may be (a row's absent `role` is `member`; absent `capabilities`, a new row takes the defaults and an existing row keeps its own). 1 to 64 distinct teams; one passkey, `412` first.")
            .json::<super::TeamRowsBody>("#TeamRowsBody")
            .ok("{}")
            .step_up()
            .errors(&[(400, "BAD_TEAMS"), (409, "FEATURE_NOT_ACTIVE")]),
        delete("/orgs/{org}/members/{account}/teams", "member_remove_teams", Session, "Remove a member from several teams, or leave them")
            .about("Each judged as `DELETE /orgs/{org}/teams/{team}/members/{account}` would judge it, and none taken unless every one may be. 1 to 64 distinct teams; one passkey, `412` first.")
            .json::<super::TeamIdsBody>("#TeamIdsBody")
            .ok("{}")
            .step_up()
            .errors(&[(400, "BAD_TEAMS"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/orgs/{org}/teams", "team_create", Session, "Create a team")
            .about("An admin, a lead of `parent`, a member with `CREATE_TEAM` at the top, or a member who holds `CREATE_CHILD_TEAM` under `parent` (a restricted team only). A package-bound session: only its own package's teams (its app team and the teams below it): `parent` one of them.")
            .json::<super::TeamBody>("#TeamBody")
            .ok("#TeamCreated"),
        get("/orgs/{org}/teams/{team}", "team_get", SignedIn, "One team")
            .ok("#TeamView")
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/teams/{team}", "team_update", Session, "Update a team")
            .about("Rename, open or restrict, move under another parent, or set its metadata (the caller's own JSON, free-form, kept as given). `parent` absent leaves the team where it is; `parent: null` (an explicit null) moves it to the top; a team id moves it under that team. A package-bound session: only its own package's teams (its app team and the teams below it): a team below its app team, moved only within.")
            .json::<super::TeamUpdateBody>("#TeamUpdateBody")
            .ok("{}"),
        delete("/orgs/{org}/teams/{team}", "team_delete", Session, "Delete a team")
            .about("With the teams below it. The instances only those teams admit (each of whose team grants goes, with no grant to the organization) are deleted with it (`apps`), unless `keepApps: true`: then they stay (`keptApps`), each without those teams' grants and with its other grants (named members, guests, `anyone`, public links), or, with none left, an empty grant list (only its owner reaches it); `keepApps` needs cluster version 1.0.0-rc.33 (`409 FEATURE_NOT_ACTIVE`). An instance another team or the organization admits only loses the deleted teams' grants either way. With such instances, the passkey is over `{ team, apps }` (plus `keepApps: true` when set), and the `412`'s details list `apps` and `keepApps`, so the approver sees what happens to them. A package-bound session: only its own package's teams (its app team and the teams below it): a team below its app team whose apps are all its package's.")
            .opt_json::<super::TeamDeleteBody>("#TeamDeleteBody")
            .ok("#TeamDeleted")
            .step_up()
            .errors(&[(404, "NOT_FOUND"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/orgs/{org}/capabilities", "org_capabilities", Session, "Set capabilities")
            .about("A member's (`account`), in a team (`team`), or a team's default; the capability bits. `412` first.")
            .json::<super::CapabilitiesBody>("#CapabilitiesBody")
            .ok("{}")
            .step_up(),
        post("/orgs/{org}/releases", "org_release_approval", AccountHolder, "Approve a mero-tee release for the organization's apps")
            .about("The owner; `412` first.")
            .json::<super::ReleaseApprovalBody>(APPROVAL)
            .ok("#ApprovedReleases")
            .step_up(),
        post("/orgs/{org}/publishers", "org_publisher", AccountHolder, "Bind a package signing key")
            .about("The key's signature over the binding proves the organization holds it; then its `MANAGE_APPLICATION` holders upgrade the apps of packages that name `publisher`. Answers the key's binding generation now.")
            .json::<super::PublisherBody>("#PublisherBody")
            .ok("#PublisherBound")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        post("/orgs/{org}/packages/{package}/upgrade", "publisher_upgrade", AccountHolder, "Upgrade every instance of a publisher's package")
            .about("An upgrade run, by an owner or admin of the publisher organization, over the apps that run the package in the organizations they are an owner or admin of (the publisher's own included), never another organization's: those are upgraded by their own owner and admins. On a cluster still below 1.0.0-rc.32, as before: by the publisher's `MANAGE_APPLICATION` holders, over every instance its publisher upgrades.")
            .json::<super::PublisherUpgradeBody>("#PublisherUpgradeBody")
            .ok("#UpgradeRun")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        get("/orgs/{org}/apps", "org_apps", SignedIn, "The organization's apps")
            .about("As `GET /auth/apps`, for the apps the caller reaches or manages; `404` to a non-member.")
            .query::<super::PageQuery>(PAGE)
            .ok("#AppsReport")
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/outbound", "org_outbound_approval", AccountHolder, "Approve outbound prefixes for the organization's apps")
            .about("The owner; `412` first. `409 EGRESS_OFF` while the organization's egress is off.")
            .json::<super::OutboundApprovalBody>(OUTBOUND)
            .ok("#OutboundApproved")
            .step_up(),
        post("/orgs/{org}/egress", "org_egress", AccountHolder, "Turn the organization's egress off or on")
            .about("`egress: false` turns egress off for every app of the organization: from each app's next outbound request on, nothing leaves the cluster for it, whatever its release declares and whatever an approval (the organization's or the operator's) says, and the organization approves no new outbound host (`409 EGRESS_OFF`). `egress: true` turns it back on, a red change made only on the consent page (`kind: setEgress`); from any other client `403 CONSENT_PAGE_REQUIRED`. The owner or an admin, with a passkey (`412` first); `403` to a member below admin, `404` to anyone outside it.")
            .json::<super::EgressBody>(EGRESS)
            .ok("#EgressSet")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (403, "CONSENT_PAGE_REQUIRED"), (404, "NOT_FOUND")]),
        post("/apps/{app}/egress", "app_set_egress", AccountHolder, "Turn one app's egress off or on")
            .about("As `POST /orgs/{org}/egress`, for this instance alone, by its organization's owner or an admin: off, it sends nothing out of the cluster whatever was approved; on again (the consent page only, `kind: setEgress` with `app`) lets it send only while its organization's egress is on too.")
            .json::<super::EgressBody>(EGRESS)
            .ok("#EgressSet")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (403, "CONSENT_PAGE_REQUIRED"), (404, "NOT_FOUND")]),
        post("/orgs/{org}/teams/{team}/members", "team_set_member", Session, "Add or change a team member")
            .about("A package-bound session: only its own package's teams (its app team and the teams below it).")
            .json::<super::TeamMemberBody>("#TeamMemberBody")
            .ok("{}")
            .step_up(),
        delete("/orgs/{org}/teams/{team}/members/{account}", "team_remove_member", Session, "Remove a team member, or leave")
            .about("A package-bound session: only its own package's teams (its app team and the teams below it).")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up(),
        post("/orgs/{org}/invites", "invite_create", Session, "Make an invite")
            .about("A token for `role` and `teams`, `uses` times within `ttlSecs`; `allApps` (an admin's) also gives every app added later; `capabilities` (bits) is what a new member gets as their own setting, no more than the creator holds (`403`), and `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.29. `412` first.")
            .json::<super::InviteBody>("#InviteBody")
            .ok("#InviteMade")
            .step_up()
            .errors(&[(400, "UNKNOWN_CAPABILITIES"), (400, "WRONG_CHALLENGE"), (403, "CANNOT_GRANT_UNHELD"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/orgs/{org}/name", "org_rename", Session, "Rename an organization")
            .about("The owner or an admin, to a name that fits as a new organization's would; `412` first.")
            .json::<super::RenameOrgBody>("#RenameOrgBody")
            .ok("#OrgRenamed")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/orgs/{org}/all-apps", "org_all_apps", Session, "Give a member every app added later")
            .about("Or stop it (`allApps: false`); apps already there are not touched. The owner or an admin; `412` first.")
            .json::<super::AllAppsBody>("#AllAppsBody")
            .ok("{}")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "FEATURE_NOT_ACTIVE")]),
        get("/orgs/{org}/invites", "invite_list", Session, "The organization's invites")
            .about("Their keys and terms, never a token: `expiresAt` Unix seconds; `createdBy` and `createdAt` are `null` on an invite from before they were kept.")
            .ok("#InviteList")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/orgs/{org}/invites/{key}", "invite_revoke", Session, "Revoke an invite")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("{}")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        get("/orgs/{org}/app-teams", "app_team_list", SignedIn, "The organization's app teams")
            .about("A member: each app team's package, id, name, `join`, member count, default capabilities, and whether the caller is in it or manages it (a guest: only their own).")
            .ok("#AppTeamList")
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/app-teams", "app_teams_scope", Session, "Give apps app teams, or open or close one")
            .about("The owner or an admin; `keep` names who stays when a team closes; `childTeams` says whether the app team's members make restricted teams below it (private channels), adding or removing `CREATE_CHILD_TEAM` (absent: as they are). Naming `join` or `childTeams` on any package leaves `shared` unset to mean no other team. `412` first.")
            .json::<super::ScopeAppsBody>("#ScopeAppsBody")
            .ok("#AppTeamsScoped")
            .step_up()
            .errors(&[(400, "TOO_MANY_PACKAGES"), (409, "FEATURE_NOT_ACTIVE")]),
        get("/orgs/{org}/backup-key", "org_backup_key", AccountHolder, "The organization's backup key")
            .about("The X25519 public key its apps' recovery-disk backups are sealed to (`key`, `null` while none is set), and how many apps that covers. Any member; `404` outside the organization.")
            .ok("#BackupKeyAnswer")
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/backup-key", "org_set_backup_key", AccountHolder, "Set the organization's backup key")
            .about("`publicKey`: 64 lowercase hex, an X25519 public key whose private half its holder keeps (never a node). Those who upgrade all the organization's apps (org-wide `MANAGE_APPLICATION`), where owners upgrade apps; replaces the key before. `412` first.")
            .json::<super::BackupKeyBody>("#BackupKeyBody")
            .ok("#BackupKeyAnswer")
            .step_up()
            .errors(&[(400, "BAD_BACKUP_KEY"), (403, "FORBIDDEN"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/orgs/{org}/recovery", "recovery_set", AccountHolder, "Set the organization's recovery key")
            .about("Set or rotate it (`publicKey`: base64url Ed25519 public key), or remove it (`publicKey: null`; an absent `publicKey` removes it too); the owner; cancels a recovery under way. `412` first.")
            .json::<super::RecoveryKeyBody>("#RecoveryKeyBody")
            .ok("{}")
            .step_up()
            .errors(&[(400, "BAD_RECOVERY_KEY")]),
        post("/orgs/{org}/recovery/start", "recovery_start", AccountHolder, "Start an organization recovery")
            .about("The recovery seed's signature handing the organization to the caller.")
            .json::<super::RecoveryStartBody>("#RecoveryStartBody")
            .ok("#RecoveryStarted")
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
            .ok("#RecoveryFinished")
            .step_up()
            .errors(&[(404, "NOT_FOUND"), (409, "RECOVERY_NOT_READY")]),
        post("/invites/{token}/redeem", "invite_redeem", Session, "Join with an invite")
            .about("No passkey prompt: the node signs the redemption with the token's key. Answers the organization joined; for an instance invite, the instance (`app`) and its organization, and `member`: whether the redeemer was named as a member of it (else a guest, never made a member). An instance invite always names a member of its organization; anyone else is refused `409 GUESTS_NOT_ALLOWED` unless the instance's release says `guests: allowed` and its organization's `allowGuests` is on, judged as the redemption applies.")
            .ok("#InviteRedeemed")
            .errors(&[(409, "GUESTS_NOT_ALLOWED")]),
        get("/orgs/{org}/upgrade-policy", "org_policy", Session, "The organization's update policy")
            .about("For its members: the policy each of its apps without one of its own follows (`source: default` until an owner or admin sets one).")
            .ok("#PolicyView")
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/upgrade-policy", "org_policy_set", AccountHolder, "Set the organization's update policy")
            .about("The owner or an admin, with a step-up over `org:<org> / __set_upgrade_policy / { app: null, policy }`; `policy: null` (or left out) drops it (the defaults). `holdHours` outside 24 to 168 is `400 BAD_UPGRADE_POLICY` before the step-up. `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.")
            .json::<super::UpgradePolicyBody>("#UpgradePolicyBody")
            .ok("#PolicyView")
            .step_up()
            .errors(&[(400, "BAD_UPGRADE_POLICY"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "FEATURE_NOT_ACTIVE")]),
        get("/orgs/{org}/updates", "org_updates", AccountHolder, "Updates waiting across an organization's apps")
            .about("For its owner and admins (`403` to other members, `404` to anyone else): its apps, by id, at most 50 read per page, that have an update available or a first approval waiting, each as `GET /apps/{app}/update` answers it with the app's `name`; `policy` is the organization's; `next` is the `after` of the next page, `null` at the end.")
            .query::<super::UpdatesQuery>("{ after?: string }")
            .ok("#OrgUpdates")
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        post("/orgs/{org}/settings", "org_settings", Session, "Change the organization's settings")
            .about("`allowGuests` (default on): whether its instances take guests, accounts named on one instance (an `account` grant) that are not members, where the instance's release allows them too (`guests: allowed` in its signed manifest); off, every guest stops reaching its instances at their next request. `allowPublicLinks` (default on): whether its instances' public links admit visitors; off, creating one is refused and existing ones admit nobody at the visitor's next call. A setting left out is kept; none is `400`. The owner or an admin; `412` first. `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.33.")
            .json::<super::OrgSettingsBody>("#OrgSettingsBody")
            .ok("#OrgSettingsSet")
            .step_up()
            .errors(&[(400, "BAD_REQUEST"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "FEATURE_NOT_ACTIVE")]),
        // ---------------------------------------------------------------- consent
        post("/consent/requests", "crate::consent_page::create", Session, "Ask the account's owner to approve")
            .about("From an app's frontend: an instance of a release or a deployment's offer (`install`: the install sheet, what gets created, where data can go and that updates wait for an approval; `outcome` names the `appId` and its `links`), an update policy (`updatePolicy`, an owner or admin of the organization), the node's updates page (`updates`, at `/updates/{request}`: the person unlocks it with their passkey, then it lists the updates waiting across the apps they approve and their organizations' policies), a release approval, an upgrade of its instance, outbound prefixes, a new organization, a new invite or joining with one, leaving an organization, a member's removal, role, teams or capabilities, an invite's revocation, an organization's name, its deletion, defaults or all-apps grants, approved on the node's own consent page (`url`), which takes the passkey. What the approval made (`orgId`, an invite's `token`, the `org` joined, a rename's `{ orgId, name }`, a deletion's `apps`) is in `/consent/{request}/result`'s `outcome`. 64 KiB at most.")
            .bytes_json::<crate::consent_page::CreateBody>(r#"{ kind: "release", scope: "account" | { org: string }, subject: { version: string, withdraw?: bool }, returnTo: string } | { kind: "upgrade", subject: { app: string, toVersion: string }, returnTo: string } | { kind: "upgradeUnit", subject: { app: string, toVersion: string }, returnTo: string } | { kind: "delegate", subject: { app: string, target: string }, returnTo: string } | { kind: "outbound", scope: "account" | { org: string }, subject: { package?: string, prefixes?: string[] }, returnTo: string } | { kind: "createOrg", subject: { name: string, capabilities?: uint }, returnTo: string } | { kind: "createInvite", subject: { org: string, role: "guest" | "member" | "admin", teams?: string[], ttlSecs?: uint, uses?: uint, allApps?: bool, capabilities?: uint }, returnTo: string } | { kind: "acceptInvite", subject: { token: string }, returnTo: string } | { kind: "leaveOrg", subject: { org: string }, returnTo: string } | { kind: "removeMember", subject: { org: string, account: string }, returnTo: string } | { kind: "setMemberRole", subject: { org: string, account: string, role: "guest" | "member" | "admin" | "owner" }, returnTo: string } | { kind: "setMemberTeams", subject: { org: string, account: string, teams: { team: string, role?: "lead" | "member" | "readOnly", capabilities?: uint }[] }, returnTo: string } | { kind: "removeMemberTeams", subject: { org: string, account: string, teams: string[] }, returnTo: string } | { kind: "revokeInvite", subject: { org: string, inviteKey: string }, returnTo: string } | { kind: "renameOrg", subject: { org: string, name: string }, returnTo: string } | { kind: "deleteOrg", subject: { org: string }, returnTo: string } | { kind: "setCapabilities", subject: { org: string, account?: string, team?: string, capabilities: uint }, returnTo: string } | { kind: "setAllApps", subject: { org: string, account: string, allApps: bool }, returnTo: string } | { kind: "setEgress", subject: { org: string, app?: string, egress: bool }, returnTo: string } | { kind: "install", subject: { package: string, version?: string, deployment?: string, name: string, org?: string, newOrg?: { name: string }, initArgs?: any }, returnTo: string } | { kind: "updatePolicy", subject: { org?: string, app?: string, policy?: { autoApply?: "never" | "noDestinations", holdHours?: uint, secondApprover?: bool } | null }, returnTo: string } | { kind: "updates", subject?: { org?: string }, returnTo: string }"#)
            .ok("#ConsentRequestMade")
            .errors(&[(400, "BAD_REQUEST"), (400, "UNKNOWN_CAPABILITIES"), (403, "CANNOT_GRANT_UNHELD"), (409, "FEATURE_NOT_ACTIVE"), (413, "TOO_LARGE")]),
        get("/consent/{request}/result", "crate::consent_page::result", Session, "What became of a consent request")
            .about("`decidedAt` (Unix seconds) is `null` while it is pending. `outcome`, once approved, is what the approval made: `{ orgId }` for `createOrg` (with `teamId`, the asking app's own team the organization starts with, when an app's frontend asked on a cluster that makes it), `{ token }` for `createInvite`, `{ org }` for `acceptInvite`, `{ orgId, name }` for `renameOrg`, `{ apps }` (the apps deleted) for `deleteOrg`; absent for the other kinds.")
            .ok("#ConsentResult")
            .errors(&[(404, "NOT_FOUND"), (503, "UNAVAILABLE")]),
        // ---------------------------------------------------------------- ops
        get("/ops/status", "ops_status", Ops, "The node and the cluster")
            .about("Counts, Raft status and governance; never app state, names or credentials. `groups`: the app groups this node hosts; `apps` and `accounts` are counts; `rollback` is this node's rollback counter, `null` on a node without one.")
            .ok("#OpsStatus"),
        get("/ops/cluster", "ops_cluster", Ops, "The cluster's identity")
            .about("The cluster's name and the identity this replica holds (hex).")
            .ok("#ClusterIdentity"),
        get("/ops/metrics", "ops_metrics", Ops, "Prometheus metrics")
            .about("Including per-app usage for billing. `--metrics-listen ADDR` also serves it as plain `GET /metrics` there.")
            .text("the Prometheus text format"),
        get("/ops/placement", "ops_placement", Ops, "Where every app's replicas are")
            .about("Ids and counts only, read after a read index (`fresh`: this node caught up with the system leader); `release` is a node's attested release; `clusterVersion` is the logged cluster version (`null` until the system leader logs one).")
            .ok("#Placement"),
        get("/ops/apps", "ops_apps", Ops, "What each app runs")
            .about("Each app's bundle, package, publisher, who holds its upgrades and release approvals, its last upgrade, approved releases, replicas' releases and `lastWriteAt`.")
            .query::<super::PageQuery>(PAGE)
            .ok("#OpsApps"),
        get("/ops/orgs", "ops_orgs", Ops, "Each organization's approved releases")
            .query::<super::PageQuery>(PAGE)
            .ok("#OpsOrgs"),
        get("/ops/served-origins", "origins::ops_served", Ops, "Custom domains to serve")
            .about("Every served origin a package's manager registered, by the package's label, never its name; `addedAt` is RFC 3339.")
            .ok("#ServedOrigins"),
        post("/ops/nodes", "ops_register_node", Ops, "Register a node")
            .about("Signed with the operator key over `cluster / __register_node / { id, addr, zone }`. The system leader adds it as a learner and promotes caught-up learners while fewer than 3 voters are left; `role: job` registers a compute node (in `pool`).")
            .json::<super::NodeBody>(r#"{ id: uint, addr: string, zone?: string, operatorProof?: #OperatorProof, role?: "replica" | "job", pool?: string }"#)
            .ok("{}")
            .operator()
            .errors(&[(400, "BAD_ADDRESS"), (400, "BAD_POOL"), (400, "POOL_FOR_JOB_NODES"), (409, "ADDRESS_TAKEN"), (400, "NODE_HOLDS_CLUSTER")]),
        delete("/ops/nodes/{id}", "ops_remove_node", Ops, "Remove a node")
            .about("One no app keeps a replica on; the system leader (elsewhere `421 NOT_LEADER`); signed over `__remove_node / { id }`.")
            .proof_body()
            .ok("#NodeRemoved")
            .operator()
            .errors(&[(400, "LAST_NODE"), (421, "NOT_LEADER")]),
        get("/ops/job-nodes/{id}/attestation", "ops_job_node_attestation", Ops, "Whether the cluster accepts a job node")
            .about("Runs the check the leader makes before it sends a job node work: a quote that passes the release policy, this cluster and node, this cluster's identity, a release not retired (not the per-app release approval). `admitted` carries the attested `release`; `refused` (final), with the reason: a 400 or 404 from the node, a 2xx that is oversized or does not decode, a failed verification, or a 400 or 404 from the collateral source on the PCK certificate or TCB info request; `unreachable` (transient), with the reason: anything else, including no answer, any other status from the node and any other collateral failure. `503` when this node's directory is stale. A verified answer is reused for up to a minute.")
            .ok("#JobNodeVerdict")
            .errors(&[(404, "NOT_FOUND"), (503, "UNAVAILABLE")]),
        get("/ops/nodes/{id}/receipt-key", "account_proof::ops_receipt_key", Ops, "A node's receipt key")
            .ok("#ReceiptKey")
            .errors(&[(404, "NOT_FOUND")]),
        post("/ops/apps/{app}/replicas", "ops_set_replicas", Ops, "Move an app's replicas")
            .about("Signed over `<app> / __set_replicas / { replicas }` (sorted); at least `min(3, nodes)` registered nodes, each on a release the app's owner approved. The set it already has answers `unchanged: true`. `status` is the app group's Raft status when the move finished (as the leader saw it).")
            .json::<super::ReplicaBody>("{ replicas: uint[], operatorProof?: #OperatorProof }")
            .ok("#ReplicasSet")
            .operator()
            .errors(&[(400, "DUPLICATE_REPLICA"), (400, "JOB_NODE"), (400, "TOO_FEW"), (400, "UNKNOWN_NODE"), (409, "MOVE_IN_PROGRESS"), (409, "RUNTIME_TOO_OLD"), (421, "NOT_LEADER")]),
        post("/ops/apps/{app}/restore", "restore::ops_restore", Ops, "Restore an app from its sealed backup")
            .about("Signed over `<app> / __restore_app / { backup, sha256 }`. The receiving node holds the backup (`backup`, else the newest, in `{backup-dir}/{app}/`) and seeds the restore; answers once every replica holds the restored state: the rows restored, the backup (`format` 0, with `merod` and `release` `null`, for one written before manifests) and the app group's Raft status. `recovery` restores from a recovery-disk file instead (in the app's holder's directory under `--recover-from`, else `--recovery-dir`), opened with the data key its holder opened from it (hex); `backup` is then the file's name and `sha256` its body's.")
            .json::<super::restore::RestoreBody>("{ backup?: string, recovery?: { file: string, key: string }, operatorProof?: #OperatorProof }")
            .ok("#AppRestored")
            .operator()
            .errors(&[(400, "BAD_BACKUP"), (400, "BAD_RECOVERY_FILE"), (400, "BAD_RECOVERY_KEY"), (400, "BAD_REQUEST"), (400, "NOT_A_REPLICA_NODE"), (400, "UNATTESTED_RECOVERY_FILE"), (403, "FORBIDDEN"), (404, "BACKUP_NOT_FOUND"), (409, "BACKUP_FROM_NEWER_RELEASE"), (409, "BUNDLE_GONE"), (409, "MOVE_IN_PROGRESS"), (409, "NO_BACKUP_DIR"), (409, "RECOVERY_SEGMENTS_MISSING"), (409, "UNKNOWN_BACKUP_FORMAT"), (409, "UPGRADE_PENDING")]),
        post("/recover/holder", "restore::recover_holder", Anyone, "Bring an organization's or account's apps back from its recovery files")
            .about("Sent by the holder's own device, over the sealed channel: its newest `holder` file (`slice`), each app's file and the code files, in the receiving node's `--recovery-dir` (or `--recover-from`) under `org-<id>/` or `account-<id>/`, each with the data key the holder opened from it with its backup key (hex). The cluster admits it with the operator's signature over `cluster / __recover_holder / { holder, slice, apps: [{ app, file, sha256 }] }` (`428` first, naming it). Where owners upgrade apps, and only for an organization, account, apps and passkeys the cluster does not hold; the receiving node seeds each app, which answers `409 APP_RESTORING` until its replicas hold it. People sign in again with the passkeys they had.")
            .json::<super::restore::RecoverHolderBody>("{ holder: string, slice: { file: string, key: string }, apps?: { file: string, key: string }[], code?: { file: string, key: string }[], operatorProof?: #OperatorProof }")
            .ok("#HolderRecovered")
            .operator()
            .errors(&[(400, "BAD_RECOVERY_FILE"), (400, "BAD_RECOVERY_KEY"), (400, "BAD_REQUEST"), (400, "NOT_A_REPLICA_NODE"), (400, "TOO_MANY_FILES"), (400, "UNATTESTED_RECOVERY_FILE"), (409, "FEATURE_NOT_ACTIVE"), (409, "NO_BACKUP_DIR"), (409, "RECOVERY_SEGMENTS_MISSING")]),
        get("/ops/recovery-files", "restore::ops_recovery_files", Ops, "The recovery files a node reads")
            .about("In `--recover-from`, else `--recovery-dir`, of `holder` (`operator`, the default; `org:<id>`; `account:<id>`): each file's name, its header as signed (JSON text) and the node's Ed25519 signature over `calimero.recovery-backup.v1\\0` and those bytes (hex), for the holder's device to check before it opens a key with its own.")
            .query::<super::restore::RecoveryFilesQuery>("{ holder?: string }")
            .ok("#RecoveryFiles")
            .errors(&[(400, "BAD_REQUEST"), (409, "NO_BACKUP_DIR")]),
        post("/ops/recover", "restore::ops_recover", Ops, "Found the cluster again from its recovery disk")
            .about("On the founder started with `--bootstrap --recover-from DIR`, its system state empty: the newest `system` file in `DIR/operator/` and the apps' `code` files, each with the data key the operator opened from it (hex). Checked as founding from backups is (governance, operator key, no node of the old cluster answering), then the system group is seeded; each app waits for `POST /ops/apps/{app}/restore { recovery }`.")
            .json::<super::restore::RecoverBody>("{ system: { file: string, key: string }, code?: { file: string, key: string }[] }")
            .ok("#SystemRecovered")
            .errors(&[(400, "BAD_RECOVERY_FILE"), (400, "BAD_RECOVERY_KEY"), (400, "TOO_MANY_FILES"), (400, "UNATTESTED_RECOVERY_FILE"), (409, "NOT_RECOVERING"), (409, "OLD_CLUSTER_ANSWERS"), (409, "OTHER_DEPLOYMENT")]),
        get("/ops/backup-key", "ops_backup_key", Ops, "The operator's backup key")
            .about("The X25519 public key every app's recovery-disk backups are sealed to where the operator upgrades apps (`key`, `null` while none is set).")
            .ok("#BackupKeyAnswer"),
        post("/ops/backup-key", "ops_set_backup_key", Ops, "Set the operator's backup key")
            .about("Signed over `operator / __set_backup_key / { publicKey }`, where the operator upgrades apps (else `403`). `publicKey`: 64 lowercase hex, an X25519 public key whose private half the operator keeps (never a node).")
            .json::<super::OpsBackupKeyBody>("{ publicKey: string, operatorProof?: #OperatorProof }")
            .ok("#BackupKeyAnswer")
            .operator()
            .errors(&[(400, "BAD_BACKUP_KEY"), (403, "FORBIDDEN"), (409, "FEATURE_NOT_ACTIVE")]),
        post("/ops/backup", "ops_backup", Ops, "Snapshot the apps this node leads")
            .about("Into `--backup-dir`. With `all=true`, every group this node holds, leader or not: what a node of a cluster that lost a majority still has, to found the cluster again from (`merod --bootstrap --restore-from`). `written`: how many snapshots it wrote.")
            .query::<super::BackupQuery>("{ all?: bool }")
            .ok("#BackupWritten"),
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
            .json::<super::DeployerBody>("#DeployerBody")
            .ok("#DeployerGranted")
            .operator(),
        delete("/ops/deployers/{key}", "ops_deployer_revoke", Ops, "Revoke a deployer")
            .proof_body()
            .ok("#DeployerRevoked")
            .operator(),
        post("/ops/deployers/{key}/revoke", "ops_deployer_revoke", Ops, "Revoke a deployer (POST)")
            .about("The same as the `DELETE`, for an HTTP client whose `DELETE` carries no body.")
            .proof_body()
            .ok("#DeployerRevoked")
            .operator(),
        post("/ops/deployments/granted", "ops_deployments_granted", Ops, "Require deployer grants")
            .about("From now on only granted deployers install; once.")
            .proof_body()
            .ok("#GovernanceSet")
            .operator()
            .errors(&[(409, "CONFLICT")]),
        post("/ops/cluster-owner", "ops_cluster_owner", Ops, "Name the cluster's owner")
            .about("The account or organization that then runs sign-up and deployers with passkeys; once.")
            .json::<super::ClusterOwnerBody>("{ owner: #Deployer, operatorProof?: #OperatorProof }")
            .ok("#ClusterOwnerNamed")
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
            .ok("#SignUpInviteList")
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
            .ok("#UpgradeApplied")
            .operator()
            .errors(&[(400, "AMBIGUOUS_COMPUTE"), (400, "BAD_REQUEST"), (409, "NOT_INSTALLED"), (409, "RELEASE_DEADLINE"), (409, "RUNTIME_TOO_OLD"), (422, "UPGRADE_FAILED")]),
        post("/ops/orgs/{org}/upgrade", "ops_org_upgrade", Ops, "An operator's upgrade run")
            .json::<super::OrgUpgradeBody>("#OrgUpgradeBody")
            .ok("#UpgradeRun")
            .operator()
            .errors(&[(404, "NOT_FOUND")]),
        post("/ops/packages", "ops_package_upload", Ops, "Install an uploaded release as the operator")
            .raw("application/octet-stream", "the `.mpk`")
            .ok("#PackageInstalled")
            .errors(&[(400, "BAD_PACKAGE"), (400, "BAD_BUNDLE"), (400, "COMPUTE_ABI_MISMATCH"), (400, "UNSUPPORTED_BUNDLE"), (413, "TOO_LARGE"), (422, "COMPILE_TIMEOUT"), (503, "BUSY")]),
        get("/ops/packages", "ops_package_list", Ops, "The releases the operator installed").ok("#PackageList"),
        post("/ops/packages/install", "ops_package_install", Ops, "Install a release from the registry as the operator")
            .json::<super::InstallBody>("{ package: string, version: string }")
            .ok("#PackageInstalled")
            .errors(&[(400, "BAD_BUNDLE"), (400, "COMPUTE_ABI_MISMATCH"), (400, "UNSUPPORTED_BUNDLE"), (413, "TOO_LARGE"), (422, "COMPILE_TIMEOUT"), (503, "BUSY")]),
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
            .ok("#OpsPools"),
        post("/ops/pools", "ops_pools_set", Ops, "Set the compute pools")
            .about("Signed over `cluster / __set_pools / { pools }`; `POOL_IN_USE` when a pool to remove still has job nodes or runs.")
            .json::<super::PoolsBody>("{ pools: { [string]: { runsPerNode: uint, maxMemoryMib: uint } }, operatorProof?: #OperatorProof }")
            .ok("{}")
            .operator()
            .errors(&[(409, "POOL_IN_USE"), (409, "UPGRADE_PENDING")]),
        get("/ops/prices", "ops_prices_get", Ops, "The price table")
            .about("In micro-units of the billing currency; `{}` until one is set.")
            .ok("#PriceTableView"),
        post("/ops/prices", "ops_prices_set", Ops, "Set the price table")
            .about("Signed over `cluster / __set_price_table / { table }`; `PRICE_VERSION` unless `version` is the current one plus one (1 for the first), `BAD_PRICES` for a zero `fuelPerVcpuSecond`.")
            .json::<super::PricesBody>("{ table: { version: uint, fuelPerVcpuSecond: uint, vcpuSecond: uint, gibSecond: uint, connectorGib: uint, storageGibMonth: uint, egressGib: uint }, operatorProof?: #OperatorProof }")
            .ok("{}")
            .operator()
            .errors(&[(400, "BAD_PRICES"), (409, "PRICE_VERSION"), (409, "UPGRADE_PENDING")]),
        // ---------------------------------------------------------------- the cluster's owner
        post("/sign-up", "sign_up_set", AccountHolder, "Set the sign-up policy")
            .about("The cluster's owner, once named, with a passkey (`412` first).")
            .json::<super::SignUpBody>("{ policy: #SignUp, operatorProof?: #OperatorProof, confirm?: #Confirm }")
            .ok("#SignUpSet")
            .step_up(),
        post("/sign-up/invites", "sign_up_invite_create", AccountHolder, "Issue a cluster sign-up invite")
            .json::<super::SignUpInviteBody>("{ uses?: uint, ttlSecs?: uint, confirm?: #Confirm }")
            .ok("#SignUpInviteMade")
            .step_up()
            .errors(&[(400, "BAD_REQUEST"), (400, "WRONG_CHALLENGE")]),
        get("/sign-up/invites", "sign_up_invite_list", AccountHolder, "Cluster sign-up invites")
            .ok("#SignUpInviteList")
            .errors(&[(403, "FORBIDDEN")]),
        delete("/sign-up/invites/{key}", "sign_up_invite_revoke", AccountHolder, "Withdraw a cluster sign-up invite")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("#InviteKey")
            .step_up(),
        post("/deployers", "deployer_grant", AccountHolder, "Grant a deployer")
            .about("The cluster's owner, with a passkey.")
            .json::<super::DeployerBody>("#DeployerBody")
            .ok("#DeployerGranted")
            .step_up(),
        get("/deployers", "deployer_list", AccountHolder, "The deployers")
            .about("The operator, or the cluster's owner: the deployments and deployers governance, the owner (`null` until named), and every grant (`until` RFC 3339 and `untilSecs`, `null` for none; `grantedAt` RFC 3339; `deployer` `null` for a key it cannot read).")
            .ok("#DeployerList")
            .errors(&[(403, "FORBIDDEN")]),
        delete("/deployers/{key}", "deployer_revoke", AccountHolder, "Revoke a deployer")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("#DeployerRevoked")
            .step_up(),
        post("/deployments", "deployment_create", AccountHolder, "Create a deployment")
            .about("A named set of releases an owner (an account or organization) offers for others to instantiate. Answers its id, the record as made (no apps yet) and its frontend host (`null` on a node with no apps domain).")
            .json::<super::DeploymentBody>("#DeploymentBody")
            .ok("#DeploymentCreated")
            .step_up()
            .errors(&[(403, "FORBIDDEN")]),
        get("/deployments", "deployment_list", SignedIn, "Deployments")
            .about("The deployments the caller manages.")
            .ok("#DeploymentList"),
        get("/deployments/{id}", "deployment_get", SignedIn, "One deployment")
            .about("As anyone may read it: its name, its owner's kind and name (`null` when unknown), and the releases it offers; never its managers or customers.")
            .ok("#PublicDeployment")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/deployments/{id}", "deployment_delete", AccountHolder, "Delete a deployment")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("#DeploymentDeleted")
            .step_up(),
        post("/deployments/{id}/apps", "deployment_offer", AccountHolder, "Offer a release in a deployment")
            .json::<super::OfferBody>("#OfferBody")
            .ok("#AppOffered")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        delete("/deployments/{id}/apps/{app}", "deployment_withdraw", AccountHolder, "Withdraw an offer")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("#AppWithdrawn")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        get("/deployments/{id}/orgs", "deployment_orgs", SignedIn, "The organizations a deployment serves")
            .about("For its managers: each organization with instances from it, its name (`null` when unknown), member counts (`activeMembers30d`: active in the last 30 days), its instances and the releases they run, and when its first was created (RFC 3339); instances outside an organization, or (below cluster version 1.0.0-rc.32) in a personal organization, are only counted (`outsideOrgs`). Never who the members are.")
            .ok("#DeploymentOrgs"),
        get("/deployments/{id}/secrets", "deployment_secrets", SignedIn, "A deployment's secrets")
            .about("For its managers, on fresh state: every secret a release the deployment offers declares, or a release one of those references (`uses`, for the apps it creates), and every one it holds a value of. Each row names the deployment's apps whose release declares it or references one that does (`apps`, empty when none does any more: the value is kept, unused, until cleared), the releases that declare it (`declaredBy`) and `set`; a set one adds its `fingerprint` (the first 16 hex digits of the value's SHA-256), `setBy` (`account:<id>`) and `setAt` (Unix seconds). Never a value.")
            .ok("#DeploymentSecrets")
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        put("/deployments/{id}/secrets/{name}", "deployment_secret_set", AccountHolder, "Set a deployment's value of a secret")
            .about("Every instance created from the deployment uses it unless the instance set its own (`app_secret_set`), and only where the secret's binding says (its host, header or body field), for an instance whose release declares the name; it wins over the operator's. Kept in the system group's state, sealed, never shown again (`deployment_secrets` answers its fingerprint) and out of every app's reach. A manager of the deployment, with a step-up over `deployment:{id} / __set_secret / { name, fingerprint }`: the value is never in the consent. `404 UNDECLARED_SECRET` unless a release the deployment offers declares the name, or a release one of those references in its signed `uses` (an installed release of the package inside the reference's range), whose value reaches the apps an instance creates with it; a name of A-Z, 0-9 and `_` and a value of 1 to 4,096 bytes (`400 BAD_SECRET`); at most 16 per deployment (`429 QUOTA_EXCEEDED`); `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.32.")
            .json::<super::DeploymentSecretBody>("#DeploymentSecretBody")
            .ok("#DeploymentSecretSet")
            .step_up()
            .errors(&[(400, "BAD_SECRET"), (403, "FORBIDDEN"), (404, "NOT_FOUND"), (404, "UNDECLARED_SECRET"), (409, "FEATURE_NOT_ACTIVE"), (429, "QUOTA_EXCEEDED")]),
        delete("/deployments/{id}/secrets/{name}", "deployment_secret_clear", AccountHolder, "Remove a deployment's value of a secret")
            .about("Its instances that set no value of their own fall back to the operator's. A manager, with a step-up over `deployment:{id} / __clear_secret / { name }`; `404` when the deployment holds no value of it.")
            .opt_json::<super::ConfirmOnlyBody>("#ConfirmOnly")
            .ok("#DeploymentSecretCleared")
            .step_up()
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "FEATURE_NOT_ACTIVE")]),
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
        "health" | "attestation" | "releases" | "publishers" | "recover" => "Public",
        "auth" => "Accounts",
        "bundles" | "packages" | "package-origins" => "Packages",
        "apps" | "pipelines" => "Apps",
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
        components, document, fields_of, operations, shapes, Body, Input, Op, Reply, ERROR_DETAILS,
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
        ("exports::", include_str!("exports.rs")),
        ("pipelines::", include_str!("pipelines.rs")),
        ("pipeline_events::", include_str!("pipeline_events.rs")),
        ("pipeline_receipt::", include_str!("pipeline_receipt.rs")),
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

    /// An operation's JSON answer is `{}`, a plain value or a named component (`#Name`), so
    /// every SDK names its type after the node's (`MemberSecurityList`, never
    /// `OrgMembers200Response`); [`every_component_answered_whole_is_a_types`] binds that
    /// component to the Rust type the handler serializes (review finding E7). A new route that
    /// writes its answer inline turns this red.
    #[test]
    fn answers_are_named_components() {
        let mut inline = BTreeSet::new();
        for op in operations() {
            let Reply::Json(shape) = op.reply else {
                continue;
            };
            match parse(shape) {
                _ if shape == "{}" => {}
                Shape::Ref(_) => {}
                // A plain value (`null`, a string, a list of them) names no type in an SDK.
                other if !names_an_object(&other) => {}
                _ => {
                    let _new = inline.insert(format!("{} {} ({})", op.method, op.path, op.handler));
                }
            }
        }
        assert!(
            inline.is_empty(),
            "answers written inline (make each a component named after its type):\n{}",
            inline.into_iter().collect::<Vec<_>>().join("\n")
        );
    }

    /// A component an operation answers as its whole success body names the Rust type the
    /// handler serializes (`Component::of`), so `response_shapes_match_their_types` holds the two
    /// together and a field added to one and not the other turns red.
    #[test]
    fn every_component_answered_whole_is_a_types() {
        let comps = components();
        let mut untyped = BTreeSet::new();
        for op in operations() {
            let Reply::Json(shape) = op.reply else {
                continue;
            };
            let Shape::Ref(name) = parse(shape) else {
                continue;
            };
            let c = comps.iter().find(|c| c.name == name).expect("a component");
            if c.ty.is_none() {
                let _new = untyped.insert(format!("#{name} ({} {})", op.method, op.path));
            }
        }
        assert!(
            untyped.is_empty(),
            "answered whole but bound to no type (`.of::<T>()`):\n{}",
            untyped.into_iter().collect::<Vec<_>>().join("\n")
        );
    }

    /// Whether an SDK generator makes a type of its own for `shape`: an object, or a list or
    /// union that holds one.
    fn names_an_object(shape: &Shape) -> bool {
        match shape {
            Shape::Object(..) => true,
            Shape::List(item) => names_an_object(item),
            Shape::OneOf(alts) => alts.iter().any(names_an_object),
            _ => false,
        }
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
            super::super::orgs::with_detail(
                confirm(),
                "then",
                json!([{ "app": "a", "method": "m", "args": { "principal": "key:k" } }]),
            ),
            super::super::auth::follow_up_failed(
                1,
                &crate::types::KeyFollowUp {
                    app: "a".to_owned(),
                    method: "m".to_owned(),
                    args: Value::Null,
                },
                &failure(&Status::AppError, &app_error, false),
            ),
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
            crate::packages::of_service(
                ApiError::from(crate::compile::CompileError::TimedOut(
                    crate::compile::COMPILE_TIMEOUT,
                )),
                "svc",
                true,
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
                // Not serialized: the answer keeps the shape clients know.
                stopped: Some("a disk error".to_owned()),
                restarts: 1,
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
