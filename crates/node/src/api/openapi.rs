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
//!   field is optional in the shape exactly when `T` accepts the body without it;
//! - each error code the handler raises with a literal code is listed for it;
//! - the committed document is the one [`document`] builds now. `UPDATE_OPENAPI=1 cargo
//!   test -p calimero-node --lib openapi` rewrites it.
//!
//! Response shapes are written here by hand from the handlers' `json!` answers, and are not
//! checked against them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};
use serde_json::{json, Map, Value};

mod shape;

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
}

/// The success answer.
enum Reply {
    Json(&'static str),
    /// Server-sent events: what they carry.
    Events(&'static str),
    /// `text/plain`.
    Text(&'static str),
    /// `application/octet-stream`.
    Bytes(&'static str),
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
    /// `428 OPERATOR_SIGNATURE_REQUIRED` first: the operator key's signature.
    operator: bool,
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
        operator: false,
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
            shape: "{ operatorProof?: #OperatorProof }",
            ty: Some(ty::<super::OpsProofBody>()),
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

    fn bytes(mut self, what: &'static str) -> Self {
        self.reply = Reply::Bytes(what);
        self
    }

    fn step_up(mut self) -> Self {
        self.step_up = true;
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
    }
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
the codes it raises itself and those its credential implies; any operation may also answer \
`400 BAD_REQUEST`, `400 REJECTED`, `421 NOT_LEADER` and `503 UNAVAILABLE`.\n\n\
**Numbers** that may exceed 2^53 (fuel, bytes) travel as decimal strings (D20).";

/// The shared shapes.
fn components() -> Vec<Component> {
    vec![
        component(
            "Error",
            "An error answer.",
            "{ status: uint, error: string, message: string, details?: any }",
        ),
        component(
            "Confirm",
            "A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same request sent again with it approves exactly that request.",
            "{ challengeId: string, assertion: #Assertion }",
        ),
        component(
            "ConfirmRequired",
            "`details` of a `412 CONFIRM_REQUIRED`: the WebAuthn challenge to sign with a passkey of the account.",
            "{ challengeId: string, challenge: b64url, rpId: string, allowCredentials: string[] }",
        ),
        component(
            "OperatorProof",
            "The operator key's Ed25519 signature (hex) over the statement a `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.",
            "{ nonce: string, signature: string }",
        ),
        component(
            "OperatorSignatureRequired",
            "`details` of a `428 OPERATOR_SIGNATURE_REQUIRED`: the statement to sign and the exact bytes (`message`, base64).",
            "{ subject: string, method: string, args: any, nonce: string, cluster: string, message: string }",
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
            "Written",
            "What the system log answered for the command this request wrote (for example the id it made).",
            "{ [string]: any }",
        ),
        component(
            "Applied",
            "An entry an app's log applied: its index, how it ended and the app's error, if any.",
            "{ index: uint, status: string, error: any }",
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
        ),
        component(
            "OrgRole",
            "A member's role in an organization.",
            r#""guest" | "member" | "admin" | "owner""#,
        ),
        component(
            "TeamRole",
            "A member's standing in one team.",
            r#""lead" | "member" | "readOnly""#,
        ),
        component(
            "TeamJoin",
            "Open (`everyone` who reaches the parent) or restricted (`invite`) membership.",
            r#""everyone" | "invite""#,
        ),
        component(
            "OutboundSelection",
            "`manifest` (every prefix the release declares) or these prefixes.",
            r#""manifest" | string[]"#,
        ),
        component(
            "Deployer",
            "An account or an organization.",
            "{ account: string } | { org: string }",
        ),
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
            "A node's attestation: the TDX quote (base64), the key it binds (hex) and what it was asked for.",
            "{ quote: string, transportKey: string, appHash: string, nonce: string, release: #Release, cluster: string, node: uint, mock: bool, receiptKey?: string, governance?: any, kms?: any }",
        ),
        component(
            "UpgradeRun",
            "An organization-wide upgrade run: the apps it targets, those done, and why each failed one failed.",
            "{ runId: string, package: string, version: string, team?: string, cascade: bool, by: string, startedAt: uint, apps: string[], done: string[], failed: { [string]: any }, finished: bool }",
        ),
        component(
            "App",
            "An app instance (a context) as the directory records it.",
            "{ appId: string, name: string, group: uint, bundle: string, owner: string, org: string | null, access: #Access, confirmMethods: string[], replicas: uint[], createdAt: uint, initArgs: any, computeBundle?: string, metadata: any, lastUpgrade?: any, release?: any }",
        ),
        component(
            "ProofOnly",
            "What an operator decision that names nothing in its body may carry: none at first, the operator's proof after the `428`.",
            "{ operatorProof?: #OperatorProof }",
        ),
    ]
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
            .about("In the clear. The node's id, the system group's leader, its release and the cluster's rollback epoch.")
            .ok("{ ok: bool, node: uint, systemLeader: uint | null, release: any, rollbackEpoch?: uint | null }"),
        get("/attestation", "attestation", Anyone, "The node's attestation")
            .about("In the clear. A TDX quote binding the node's transport key (or, with `purpose=receipts`, its receipt key and the governance; with `purpose=kms`, what it verified of its KMS) to `nonce`; `app`/`appHash` bind an app's bundle hash. A client checks it before it seals anything to the key.")
            .query::<super::AttestQuery>(r#"{ nonce: string, app?: string, appHash?: string, purpose?: "receipts" | "kms" }"#)
            .ok("#AttestationDoc")
            .errors(&[(400, "BAD_NONCE"), (400, "BAD_HASH"), (404, "NOT_FOUND")]),
        get("/releases", "releases_public", Anyone, "The cluster's releases")
            .about("The mero-tee releases the cluster's nodes run and the deadlines set on them.")
            .ok("{ [string]: any }"),
        get("/publishers/{signer}", "publisher_binding", Anyone, "A signing key's organization")
            .about("The organization a package signing key (`did:key`) is bound to, if any.")
            .ok("{ [string]: any }"),
        get("/sign-up", "sign_up_public", Anyone, "The sign-up policy")
            .about("Who may create an account here, the cluster's name and the tag a sign-in uses to pick the person's passkey for it.")
            .ok("{ policy: #SignUp, orgInvites: bool, signUpInvites: bool, inviteRequired: bool, cluster: string, passkeyTag: string }"),
        // ---------------------------------------------------------------- accounts
        post("/auth/passkey/register/begin", "register_begin", Anyone, "Start a sign-up")
            .about("Creation options for a new passkey. `invite` is an organization or cluster sign-up invite token, which the policy may require; `403 SIGN_UP_REFUSED` when the policy would refuse the sign-up.")
            .json::<super::RegisterBegin>("{ name?: string, origin?: string, invite?: string }")
            .ok("{ challengeId: string, options: any }")
            .errors(&[(403, "SIGN_UP_REFUSED"), (429, "RATE_LIMITED")]),
        post("/auth/passkey/register/finish", "register_finish", Anyone, "Finish a sign-up")
            .about("The new passkey's registration. Answers `412 CONFIRM_REQUIRED` first: the new passkey signs the account it creates; the retry with `confirm` (and the same `invite`) opens a session, and redeems `invite` for the new account.")
            .json::<super::RegisterFinish>("{ challengeId: string, registration: #Registration, invite?: string, confirm?: #Confirm }")
            .ok("#Session")
            .step_up()
            .errors(&[(400, "BAD_REGISTRATION"), (400, "WRONG_CHALLENGE"), (401, "PASSKEY_UNKNOWN_HERE"), (403, "SIGN_UP_REFUSED")]),
        post("/auth/passkey/login/begin", "login_begin", Anyone, "Start a sign-in")
            .about("A WebAuthn challenge for any passkey of the cluster. `origin` is an app frontend's registered origin, whose rp-id the ceremony then uses.")
            .opt_json::<super::LoginBegin>("{ origin?: string }")
            .ok("{ challengeId: string, options: any }")
            .errors(&[(429, "RATE_LIMITED")]),
        post("/auth/passkey/login/finish", "login_finish", Anyone, "Finish a sign-in")
            .json::<super::AssertionBody>("{ challengeId: string, assertion: #Assertion }")
            .ok("#Session")
            .errors(&[(400, "WRONG_CHALLENGE"), (400, "UNKNOWN_CHALLENGE"), (400, "EXPIRED_CHALLENGE"), (401, "PASSKEY_UNKNOWN_HERE")]),
        post("/auth/passkeys/add/begin", "passkey_add_begin", AccountHolder, "Start adding a passkey")
            .about("Creation options for another passkey (a device) on the caller's account.")
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
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("{}")
            .step_up(),
        post("/auth/recovery", "recovery_create", AccountHolder, "Make a recovery code")
            .about("Shown once, replacing any; `412` first.")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("{ code: string }")
            .step_up()
            .errors(&[(400, "WRONG_CHALLENGE")]),
        delete("/auth/recovery", "recovery_remove", AccountHolder, "Remove the recovery code")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("{}")
            .step_up(),
        post("/auth/recovery/begin", "account_recovery_begin", Anyone, "Start an account recovery")
            .about("Signed out, with the recovery code: creation options for the passkey that will replace the lost ones.")
            .json::<super::RecoveryBegin>("{ code: string }")
            .ok("{ challengeId: string, options: any }")
            .errors(&[(429, "RATE_LIMITED")]),
        post("/auth/recovery/start", "account_recovery_start", Anyone, "Spend the recovery code")
            .about("Logs the new passkey as pending; it may finish the recovery once `readyAt` passed (72 h). `401` for a wrong or spent code.")
            .json::<super::RecoveryStart>("{ challengeId: string, code: string, registration: #Registration }")
            .ok("{ accountId: string, credentialId: string, startedAt: uint, readyAt: uint }")
            .errors(&[(400, "BAD_REGISTRATION"), (400, "WRONG_CHALLENGE"), (401, "UNAUTHENTICATED"), (409, "RECOVERY_PENDING")]),
        post("/auth/recovery/cancel", "account_recovery_cancel", AccountHolder, "Cancel an account recovery")
            .about("With a passkey the account has; `412` first.")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("{}")
            .step_up(),
        post("/auth/recovery/finish", "account_recovery_finish", Anyone, "Finish an account recovery")
            .about("Approved by the new passkey once `readyAt` passed (`412` first): adds it and ends every session of the account.")
            .json::<super::RecoveryFinish>("{ accountId?: string, confirm?: #Confirm }")
            .ok("#Session")
            .step_up()
            .errors(&[(400, "WRONG_CHALLENGE"), (401, "UNAUTHENTICATED"), (409, "RECOVERY_NOT_READY")]),
        post("/auth/refresh", "refresh", Anyone, "Refresh a session")
            .about("A new session and the refresh token to use next (the same one until it is an hour old). A reused, superseded token ends the sessions it opened.")
            .json::<super::RefreshBody>("{ refreshToken: string }")
            .ok("#Session")
            .errors(&[(401, "UNAUTHENTICATED"), (503, "UNAVAILABLE")]),
        post("/auth/logout", "logout", Session, "End this session").ok("{}"),
        post("/auth/logout-all", "logout_all", AccountHolder, "End every session of the account")
            .ok("#Written"),
        get("/auth/sessions", "sessions_list", AccountHolder, "The account's live sessions")
            .about("Latest active first, at most 256.")
            .ok("{ sessions: { session: string, passkey: string | null, app: string | null, origin: string | null, openedAt: uint, lastActiveAt: uint, current: bool }[] }"),
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
            .ok("#Written")
            .step_up(),
        post("/auth/outbound", "account_outbound_approval", AccountHolder, "Approve outbound prefixes for the account's apps")
            .json::<super::OutboundApprovalBody>(OUTBOUND)
            .ok("#Written")
            .step_up(),
        get("/auth/me", "me", SignedIn, "The caller's account")
            .about("The account, its passkeys (with `addedAt` and `lastSignInAt`), organizations, approvals and recovery state, and how the caller came (`via`).")
            .ok(r#"{ accountId: string, name: string, passkeys: { id: string, signCount: uint, label: string | null, addedAt: uint | null, lastSignInAt: uint | null }[], orgs: { orgId: string, name: string, role: #OrgRole }[], via: "session" | "apiKey", passkeyId: string | null, app: string | null, origin: string | null, approvedReleases: any, outboundApprovals: any, recoveryCode: any, recoveryPending: any }"#)
            .errors(&[(404, "NOT_FOUND")]),
        post("/auth/account-proof", "account_proof::prove", Session, "Prove the account to another party")
            .about("A statement signed with this node's receipt key that the caller's passkey session asked to bind `nonce` for `audience`.")
            .json::<super::account_proof::ProofBody>("{ nonce: string, audience: string }")
            .ok("{ [string]: any }")
            .errors(&[(400, "BAD_AUDIENCE"), (400, "BAD_NONCE")]),
        get("/auth/apps", "account_apps", SignedIn, "The account's own apps")
            .about("Outside any organization (a package-bound session: its package's): each app's release, the newer releases it may upgrade to, its last upgrade, its replicas' releases and `lastWriteAt`.")
            .query::<super::PageQuery>(PAGE)
            .ok("{ scope: any, governance: any, approvedReleases: any, clusterReleases: any, apps: any[], next: string | null }"),
        get("/auth/usage", "usage", SignedIn, "The account's usage against its quotas")
            .about("Apps, bundle bytes, API keys and owned organizations; what today's runs of the apps the account develops used (decimal strings).")
            .ok("{ bundleBytes: u64, developerFuelToday: u64, developerWritesToday: u64, [string]: any }"),
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
        get("/auth/apikeys", "apikey_list", AccountHolder, "The account's API keys").ok("{ keys: any[] }"),
        delete("/auth/apikeys/{key}", "apikey_revoke", AccountHolder, "Revoke an API key")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
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
            .ok("{ package: string, version: string, [string]: any }")
            .errors(&[(400, "BAD_PACKAGE"), (400, "BAD_BUNDLE"), (403, "UPLOADS_DISABLED"), (403, "DEPLOY_NOT_GRANTED"), (413, "TOO_LARGE")]),
        get("/packages", "package_list", SignedIn, "Installed packages")
            .about("Only the releases the caller may see.")
            .ok("{ packages: any[] }"),
        post("/packages/install", "package_install", Installer, "Install a release from the registry")
            .json::<super::InstallBody>("{ package: string, version: string }")
            .ok("{ package: string, version: string, [string]: any }")
            .errors(&[(400, "BAD_PACKAGE"), (400, "NO_REGISTRY"), (403, "DEPLOY_NOT_GRANTED")]),
        get("/packages/{package}", "package_get", SignedIn, "One package")
            .about("The versions the caller may see; `404` when none.")
            .ok("{ [string]: any }")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/packages/{package}", "package_uninstall", AccountHolder, "Uninstall a package")
            .about("Every version; refused while an app runs one.")
            .ok("#Written"),
        delete("/packages/{package}/{version}", "package_version_uninstall", AccountHolder, "Uninstall one version")
            .ok("#Written"),
        get("/package-origins/{package}", "origins::list", SignedIn, "A package's frontend origins")
            .about("The custom origins (and rp-ids) the package's manager registered for its frontend.")
            .ok("{ package: string, origins: any[] }"),
        post("/package-origins/{package}", "origins::add", AccountHolder, "Register a frontend origin")
            .about("By whoever manages the package, with a passkey step-up over the change.")
            .json::<super::origins::AddOriginBody>("{ origin: string, rpId?: string, served?: bool, confirm?: #Confirm }")
            .ok("#Written")
            .step_up()
            .errors(&[(400, "BAD_RP_ID"), (429, "QUOTA_EXCEEDED")]),
        delete("/package-origins/{package}", "origins::remove", AccountHolder, "Remove a frontend origin")
            .json::<super::origins::RemoveOriginBody>("{ origin: string, confirm?: #Confirm }")
            .ok("#Written")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        // ---------------------------------------------------------------- apps
        post("/apps", "app_create", Installer, "Create an app instance")
            .about("From a release (`package`, `version`, `service`), a deployment's offer (`deployment`), or an uploaded `bundle`; in an organization (`org`) or the caller's own. `initArgs` go to the app's `init`.")
            .json::<super::CreateAppBody>("{ name?: string, bundle?: string, package?: string, version?: string, service?: string, org?: string, access?: #Access, initArgs?: any, metadata?: any, deployment?: string }")
            .ok("{ appId: string, contextId: string, group: uint, replicas: uint[], compute?: any }")
            .errors(&[(400, "BAD_REQUEST"), (400, "NOT_OFFERED"), (400, "VERSION_REQUIRED"), (400, "NO_SUCH_SERVICE"), (400, "AMBIGUOUS_COMPUTE"), (409, "RELEASE_DEADLINE"), (422, "INIT_FAILED"), (429, "QUOTA_EXCEEDED"), (503, "UNAVAILABLE")]),
        get("/apps", "app_list", SignedIn, "Apps the caller reaches")
            .about("Anyone may list public apps.")
            .query::<super::AppListQuery>("{ package?: string, bundle?: string }")
            .ok("{ apps: #App[] }"),
        get("/apps/{app}", "app_get", SignedIn, "One app")
            .about("`{app}` is an app id or its context id (`hex(SHA-256(app_id))`), here and on every `/apps/{app}` route.")
            .ok("#App")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/apps/{app}", "app_delete", Session, "Delete an app")
            .about("The owner or an organization admin.")
            .ok("{ appId: string, deleted: bool }"),
        post("/apps/{app}/metadata", "app_metadata", SignedIn, "Set an app's metadata")
            .about("The owner or an organization admin; 16 KiB at most.")
            .json::<super::MetadataBody>("{ metadata: any }")
            .ok("{}")
            .errors(&[(413, "TOO_LARGE")]),
        get("/apps/{app}/storage", "app_storage", AppAccess, "What an app stores")
            .about("Keys, bytes and blobs held, and `lastWriteAt` (Unix seconds of its last committed write, `null` before one).")
            .ok("{ keys: uint, bytes: uint, blobs: uint, blobBytes: uint, lastWriteAt: uint | null }"),
        get("/apps/{app}/usage", "app_usage", SignedIn, "An app's runs per day")
            .about("The owner or an organization admin: `today` and the 30 `days` before it, figures as decimal strings; `wallMs` is information only.")
            .ok("{ today: any, days: { date: string, runs: u64, fuel: u64, wallMs: u64, writes: u64, bytesWritten: u64, events: u64 }[] }")
            .errors(&[(403, "FORBIDDEN")]),
        get("/apps/{app}/abi", "app_abi", AppAccess, "An app's ABI")
            .about("The ABI its bundle embeds: methods, events, errors and state.")
            .ok("{ bundle: string, abi: any }"),
        get("/apps/{app}/runs", "app_runs", AppAccess, "An app's compute runs")
            .query::<super::RunsQuery>("{ limit?: string, before?: string }")
            .ok("{ [string]: any }")
            .errors(&[(400, "BAD_LIMIT"), (404, "NOT_FOUND")]),
        get("/apps/{app}/runs/{run}", "app_run", AppAccess, "One compute run")
            .about("Its record without arguments or keys: method, state, `cancelRequested`, `startTsMs`, attempts, input and output ids (sizes and SHA-256 only for callers who may list the app's files), usage, and `progress` as the leader last polled it.")
            .ok("{ [string]: any }")
            .errors(&[(404, "NOT_FOUND")]),
        get("/apps/{app}/presence", "presence_get", AppAccess, "Who is present")
            .ok("{ present: any[] }"),
        post("/apps/{app}/presence", "presence_publish", AppAccess, "Publish presence")
            .about("The caller's presence state, in sequence; not logged.")
            .json::<super::PresenceBody>("{ seq: uint, state?: string }")
            .ok("{}")
            .errors(&[(400, "BAD_STATE"), (413, "TOO_LARGE")]),
        get("/apps/{app}/presence/stream", "presence_stream", AppAccess, "Presence as server-sent events")
            .events("a `snapshot`, then `presence` diffs"),
        post("/apps/{app}/intents", "intents", AppAccess, "Call a method")
            .about("A logged call: the result, the events of the call the caller receives, and the answering node's receipt. A `confirm` method answers `412` first. An app error is `422 APP_ERROR` with `details { index, code, fields }`; a trap `422 TRAP`.")
            .json::<super::IntentBody>("{ method: string, args?: any, idempotencyKey?: string, confirm?: #Confirm }")
            .ok("{ index: uint, result: any, events: { kind: string, data: any }[], receipt: #Receipt }")
            .errors(&[(403, "FORBIDDEN"), (404, "NOT_FOUND"), (409, "RELEASE_DEADLINE"), (412, "CONFIRM_REQUIRED"), (413, "ARGS_TOO_LARGE"), (422, "APP_ERROR"), (422, "TRAP"), (429, "QUOTA_EXCEEDED"), (503, "UNAVAILABLE")]),
        get("/apps/{app}/attestation", "app_attestation", AppAccess, "Every replica's receipt-key attestation")
            .query::<super::AttestQuery>(r#"{ nonce: string, app?: string, appHash?: string, purpose?: "receipts" | "kms" }"#)
            .ok("{ appId: string, replicas: any[] }"),
        post("/apps/{app}/views", "views", AppAccess, "Read with a view method")
            .about("Read on a caught-up replica, or (`consistency: \"stale\"`) on the one answering; `minIndex` waits until the replica applied it (`503 NOT_CAUGHT_UP`).")
            .json::<super::ViewBody>(r#"{ method: string, args?: any, consistency?: "stale", minIndex?: uint }"#)
            .ok("{ index: uint, result: any, receipt: #Receipt }")
            .errors(&[(422, "APP_ERROR"), (422, "TRAP"), (503, "NOT_CAUGHT_UP")]),
        get("/apps/{app}/events", "events", AppAccess, "An app's events as server-sent events")
            .about("Committed events from index `from`, then live ones, each to its audience.")
            .query::<super::EventsQuery>("{ from?: uint }")
            .events("each committed event the caller is in the audience of")
            .errors(&[(429, "RATE_LIMITED"), (503, "UNAVAILABLE")]),
        post("/apps/{app}/upgrade", "app_upgrade", AccountHolder, "Upgrade an app instance")
            .about("To a release (`version`, of the app's own package unless `package` names another) or a `bundle`, code only: the state stays and is read by the new code. Whoever the package's manifest names (`upgrades`: the owner, or its publisher's `MANAGE_APPLICATION` holders), with a step-up over `{ bundle, migrate, manifest }`; an upgrade key signs in place of it. A refused or failed upgrade leaves the app on its release and answers `422 UPGRADE_FAILED` with the app log's error in `details` (`MigrationsNotSupported` when `migrate` is named: there are no migrations in this version).")
            .json::<super::UpgradeBody>("{ bundle?: string, package?: string, version?: string, service?: string, confirm?: #Confirm, migrate?: string }")
            .ok("{ index: uint, status: string, error: any, compute: bool }")
            .step_up()
            .errors(&[(400, "VERSION_REQUIRED"), (400, "BAD_BUNDLE"), (400, "AMBIGUOUS_COMPUTE"), (403, "FORBIDDEN"), (409, "CONFLICT"), (409, "RELEASE_DEADLINE"), (422, "UPGRADE_FAILED")]),
        post("/apps/{app}/access", "app_access", Session, "Set who may reach an app")
            .json::<super::AccessBody>("{ access: #Access }")
            .ok("{}"),
        post("/apps/{app}/transfer", "app_transfer", AccountHolder, "Hand an app to another account")
            .about("The owner (for an organization's app, while a member and in the app's team), or an organization admin; an organization's app only to a member.")
            .json::<super::TransferBody>("{ to: string, confirm?: #Confirm }")
            .ok("#Written")
            .step_up(),
        post("/apps/{app}/move", "app_move", AccountHolder, "Move an app to another organization")
            .about("Its data where it is; its files, runs and quotas go to the target, `org` or app-team access becomes the target's app team of its package. `bringMembers` adds the source's members who reach it, capped at `member`.")
            .json::<super::MoveBody>("{ toOrg: string, bringMembers?: bool, confirm?: #Confirm }")
            .ok("{ appId: string, from: string, org: string, access: #Access, created: bool, added: any, revokedInvites: any }")
            .step_up(),
        post("/apps/{app}/blobs", "blob_upload", AppAccess, "Upload a blob")
            .about("A plain blob (`sealed=true` is refused: sealed files go through `/files`).")
            .query::<super::BlobUploadQuery>("{ sealed?: bool }")
            .raw("application/octet-stream", "the blob")
            .ok("{ blobId: string, id: string, size: uint, index: uint }")
            .errors(&[(400, "USE_FILES_API"), (413, "TOO_LARGE"), (429, "QUOTA_EXCEEDED")]),
        get("/apps/{app}/blobs", "blob_list", AppAccess, "List an app's blobs")
            .about("Plain blobs only.")
            .ok("{ blobs: { blobId: string, id: string, size: uint }[] }"),
        get("/apps/{app}/blobs/{id}", "blob_download", AppAccess, "Download a blob")
            .about("`{id}` is base58 or hex; `grant` is a link the app made (`BlobRef::grant`).")
            .query::<super::BlobGrant>("{ grant?: string }")
            .bytes("the blob, with its MIME type")
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
            .ok("{ uploadId: string, partBytes: uint, parts: uint, expiresAt: uint, next: uint }"),
        delete("/apps/{app}/files/uploads/{upload}", "upload_abort", SignedIn, "Abort an upload")
            .ok("{ [string]: any }"),
        put("/apps/{app}/files/uploads/{upload}/parts/{k}", "upload_part", SignedIn, "Send one part")
            .about("Part `k` (from 0), raw bytes; a part out of order answers the next one expected in `details.next`.")
            .raw("application/octet-stream", "the part")
            .ok("{ next: uint }")
            .errors(&[(400, "BAD_REQUEST"), (400, "BAD_PART")]),
        post("/apps/{app}/files/uploads/{upload}/complete", "upload_complete", SignedIn, "Commit an upload")
            .about("Charges and commits the file; a retry answers the same file.")
            .ok("{ blobId: string, id: string, size: uint, sha256: string, index: uint }"),
        get("/apps/{app}/files/{id}", "file_info", SignedIn, "One sealed file")
            .about("Its plaintext size and SHA-256, never its key.")
            .ok("{ blobId: string, id: string, size: uint, sha256: string }")
            .errors(&[(404, "NOT_FOUND")]),
        get("/apps/{app}/xcalls", "app_xcalls", SignedIn, "An app's cross-app calls")
            .about("The owner or an organization admin: calls waiting for delivery and dead letters.")
            .ok("{ pending: any[], deadLetters: any[] }")
            .errors(&[(403, "FORBIDDEN"), (503, "UNAVAILABLE")]),
        // ---------------------------------------------------------------- organizations
        post("/orgs", "org_create", Session, "Create an organization")
            .about("`recoveryKey` is the public half of an organization recovery key.")
            .json::<super::OrgBody>("{ name: string, recoveryKey?: string }")
            .ok("{ orgId: string }")
            .errors(&[(429, "QUOTA_EXCEEDED")]),
        get("/orgs/{org}", "org_get", SignedIn, "One organization")
            .about("Its members, teams (those the caller reaches or manages) and metadata.")
            .ok("{ orgId: string, name: string, owner: string, members: any, teams: any, metadata: any, [string]: any }")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/orgs/{org}", "org_delete", AccountHolder, "Delete an organization")
            .about("With its teams, invites and apps; the owner.")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("#Written")
            .step_up(),
        post("/orgs/{org}/metadata", "org_metadata", SignedIn, "Set an organization's metadata")
            .json::<super::MetadataBody>("{ metadata: any }")
            .ok("{}"),
        get("/orgs/{org}/usage", "org_usage", SignedIn, "An organization's usage")
            .about("An admin: what today's runs of the apps the organization develops used (decimal strings) and its quotas; a member gets `403`.")
            .ok("{ apps: any, developerFuelToday: u64, developerWritesToday: u64, quotas: any }")
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
            .json::<super::MetadataBody>("{ metadata: any }")
            .ok("{}"),
        get("/orgs/{org}/members", "org_members", AccountHolder, "Members' lockout risk")
            .about("`detail=security`, the owner or an admin: each member's passkeys (`1`, `2` or `\"3+\"`), recovery code, last active UTC day and sessions. `403` below admin, `404` outside the organization.")
            .query::<super::MembersQuery>(r#"{ detail?: "security" }"#)
            .ok("{ orgId: string, members: { account: string, name: string, role: #OrgRole, teams: any, passkeys: any, recoveryCode: bool, lastActiveDay: string | null, sessions: uint }[] }")
            .errors(&[(400, "BAD_REQUEST"), (403, "FORBIDDEN"), (404, "NOT_FOUND")]),
        post("/orgs/{org}/members", "org_set_member", Session, "Set a member's role")
            .about("`owner` moves ownership. `412` first.")
            .json::<super::MemberBody>("{ account: string, role: #OrgRole, confirm?: #Confirm }")
            .ok("{}")
            .step_up(),
        delete("/orgs/{org}/members/{account}", "org_remove_member", Session, "Remove a member, or leave")
            .about("`412` first; the retry's JSON body is `{ confirm }`.")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("{}")
            .step_up(),
        post("/orgs/{org}/teams", "team_create", Session, "Create a team")
            .about("An admin, a lead of `parent`, a member with `CREATE_TEAM` at the top, or a member who holds `CREATE_CHILD_TEAM` under `parent` (a restricted team only).")
            .json::<super::TeamBody>("{ name: string, parent?: string, join: #TeamJoin }")
            .ok("{ teamId: string }"),
        get("/orgs/{org}/teams/{team}", "team_get", SignedIn, "One team")
            .ok("{ [string]: any }")
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/teams/{team}", "team_update", Session, "Update a team")
            .about("Rename, open or restrict, move under another parent (`parent: null`: to the top), or set its metadata.")
            .json::<super::TeamUpdateBody>("{ name?: string, join?: #TeamJoin, parent?: string | null, metadata?: any }")
            .ok("{}"),
        delete("/orgs/{org}/teams/{team}", "team_delete", Session, "Delete a team")
            .about("With the teams and apps below it.")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
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
            .ok("#Written")
            .step_up(),
        post("/orgs/{org}/publishers", "org_publisher", AccountHolder, "Bind a package signing key")
            .about("The key's signature over the binding proves the organization holds it; then its `MANAGE_APPLICATION` holders upgrade the apps of packages that name `publisher`.")
            .json::<super::PublisherBody>("{ signerId: string, publicKey: string, signature: string, confirm?: #Confirm }")
            .ok("{ signer: string, approval: any }")
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
            .ok("{ scope: any, governance: any, approvedReleases: any, clusterReleases: any, apps: any[], next: string | null }")
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/outbound", "org_outbound_approval", AccountHolder, "Approve outbound prefixes for the organization's apps")
            .about("The owner; `412` first.")
            .json::<super::OutboundApprovalBody>(OUTBOUND)
            .ok("#Written")
            .step_up(),
        post("/orgs/{org}/teams/{team}/members", "team_set_member", Session, "Add or change a team member")
            .json::<super::TeamMemberBody>("{ account: string, lead?: bool, role?: #TeamRole, capabilities?: uint, confirm?: #Confirm }")
            .ok("{}")
            .step_up(),
        delete("/orgs/{org}/teams/{team}/members/{account}", "team_remove_member", Session, "Remove a team member, or leave")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("{}")
            .step_up(),
        post("/orgs/{org}/invites", "invite_create", Session, "Make an invite")
            .about("A token for `role` and `teams`, `uses` times within `ttlSecs`. `412` first.")
            .json::<super::InviteBody>("{ role: #OrgRole, teams?: string[], ttlSecs?: uint, uses?: uint, confirm?: #Confirm }")
            .ok("{ token: string }")
            .step_up()
            .errors(&[(400, "WRONG_CHALLENGE")]),
        get("/orgs/{org}/invites", "invite_list", Session, "The organization's invites")
            .about("Their keys and terms, never a token.")
            .ok("{ [string]: any }")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/orgs/{org}/invites/{key}", "invite_revoke", Session, "Revoke an invite")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("{}")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        get("/orgs/{org}/app-teams", "app_team_list", SignedIn, "The organization's app teams")
            .about("A member: each app team's package, id, name, `join`, member count, default capabilities, and whether the caller is in it or manages it (a guest: only their own).")
            .ok("{ mode: any, appTeams: any[] }")
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/app-teams", "app_teams_scope", Session, "Give apps app teams, or open or close one")
            .about("The owner or an admin; `keep` names who stays when a team closes. `412` first.")
            .json::<super::ScopeAppsBody>("{ apps?: { package: string, name?: string, instances?: string[], join?: #TeamJoin, keep?: string[] }[], shared?: bool, confirm?: #Confirm }")
            .ok("{ [string]: any }")
            .step_up()
            .errors(&[(400, "TOO_MANY_PACKAGES")]),
        post("/orgs/{org}/recovery", "recovery_set", AccountHolder, "Set the organization's recovery key")
            .about("Set, rotate or (`null`) remove it; the owner; cancels a recovery under way. `412` first.")
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
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("{}")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        post("/orgs/{org}/recovery/finish", "recovery_finish", AccountHolder, "Finish an organization recovery")
            .about("The account that started it, once `readyAt` passed; `412` first.")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("{ org: string, previousOwners: any }")
            .step_up()
            .errors(&[(404, "NOT_FOUND"), (409, "RECOVERY_NOT_READY")]),
        post("/invites/{token}/redeem", "invite_redeem", Session, "Join with an invite")
            .about("No passkey prompt: the node signs the redemption with the token's key.")
            .ok("{ org: string, [string]: any }"),
        // ---------------------------------------------------------------- consent
        post("/consent/requests", "crate::consent_page::create", Session, "Ask the account's owner to approve")
            .about("From an app's frontend: a release approval, an upgrade of its instance, or outbound prefixes, approved on the node's own consent page (`url`), which takes the passkey. 64 KiB at most.")
            .raw("application/json", r#"`{ kind: "release", scope, subject: { version, withdraw? }, returnTo }`, `{ kind: "upgrade", subject: { app, toVersion }, returnTo }` or `{ kind: "outbound", scope, subject: { package?, prefixes? }, returnTo }`; `scope` is `"account"` or `{ "org": id }`"#)
            .ok("{ requestId: string, url: string, code: string, expiresAt: uint }")
            .errors(&[(400, "BAD_REQUEST"), (413, "TOO_LARGE")]),
        get("/consent/{request}/result", "crate::consent_page::result", Session, "What became of a consent request")
            .ok("{ [string]: any }")
            .errors(&[(404, "NOT_FOUND"), (503, "UNAVAILABLE")]),
        // ---------------------------------------------------------------- ops
        get("/ops/status", "ops_status", Ops, "The node and the cluster")
            .about("Counts, Raft status and governance; never app state, names or credentials.")
            .ok("{ node: any, system: any, governance: any, groups: any, nodes: any, apps: any, accounts: any, rollback?: any }"),
        get("/ops/cluster", "ops_cluster", Ops, "The cluster's identity").ok("{ cluster: any, identity: any }"),
        get("/ops/metrics", "ops_metrics", Ops, "Prometheus metrics")
            .about("Including per-app usage for billing. `--metrics-listen ADDR` also serves it as plain `GET /metrics` there.")
            .text("the Prometheus text format"),
        get("/ops/placement", "ops_placement", Ops, "Where every app's replicas are")
            .about("Ids and counts only, read after a read index (`fresh`: this node caught up with the system leader); `release` is a node's attested release.")
            .ok("{ fresh: bool, apps: { app: string, group: uint, replicas: uint[] }[], nodes: { id: uint, zone: string, release: string | null, apps: uint }[] }"),
        get("/ops/apps", "ops_apps", Ops, "What each app runs")
            .about("Each app's bundle, package, publisher, who holds its upgrades and release approvals, its last upgrade, approved releases, replicas' releases and `lastWriteAt`.")
            .query::<super::PageQuery>(PAGE)
            .ok("{ fresh: bool, governance: any, apps: any[], next: string | null }"),
        get("/ops/orgs", "ops_orgs", Ops, "Each organization's approved releases")
            .query::<super::PageQuery>(PAGE)
            .ok("{ fresh: bool, governance: any, orgs: { org: string, approvedReleases: any }[], next: string | null }"),
        get("/ops/served-origins", "origins::ops_served", Ops, "Custom domains to serve")
            .about("Every served origin a package's manager registered, by the package's label, never its name.")
            .ok("{ fresh: bool, origins: { origin: string, host: string, label: string, addedAt: uint }[] }"),
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
            .about("Signed over `<app> / __set_replicas / { replicas }` (sorted); at least `min(3, nodes)` registered nodes, each on a release the app's owner approved. The set it already has answers `unchanged: true`.")
            .json::<super::ReplicaBody>("{ replicas: uint[], operatorProof?: #OperatorProof }")
            .ok("{ status: string, unchanged: bool }")
            .operator()
            .errors(&[(400, "DUPLICATE_REPLICA"), (400, "JOB_NODE"), (400, "TOO_FEW"), (400, "UNKNOWN_NODE"), (409, "MOVE_IN_PROGRESS"), (421, "NOT_LEADER")]),
        post("/ops/backup", "ops_backup", Ops, "Snapshot the apps this node leads")
            .about("Into `--backup-dir`.")
            .ok("{ written: any }"),
        post("/ops/releases", "ops_release", Ops, "Approve a mero-tee release for every app")
            .about("Where the operator holds release approvals.")
            .json::<super::OpsReleaseBody>("{ version: string, approved?: bool, operatorProof?: #OperatorProof }")
            .ok("#Written")
            .operator(),
        post("/ops/releases/{version}/deadline", "ops_release_deadline", Ops, "Set a release deadline")
            .about("After `after` (RFC 3339), apps whose approver has not moved past the release may start no new work.")
            .json::<super::OpsDeadlineBody>("{ after?: string, operatorProof?: #OperatorProof }")
            .ok("#Written")
            .operator(),
        delete("/ops/releases/{version}/deadline", "ops_release_deadline_lift", Ops, "Lift a release deadline")
            .proof_body()
            .ok("#Written")
            .operator(),
        post("/ops/deployers", "ops_deployer_grant", Ops, "Grant a deployer")
            .about("An account or organization that may install where deployments are granted, `until` an RFC 3339 time.")
            .json::<super::DeployerBody>("{ deployer: #Deployer, until?: string, operatorProof?: #OperatorProof, confirm?: #Confirm }")
            .ok("{ key: string, deployer: #Deployer, grant: any }")
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
            .ok("{ governance: any }")
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
            .ok("#Written")
            .operator(),
        post("/ops/sign-up/invites", "ops_sign_up_invite_create", Ops, "Issue a cluster sign-up invite")
            .json::<super::OpsSignUpInviteBody>("{ token: string, uses?: uint, expiresAt: string, operatorProof?: #OperatorProof }")
            .ok("{ [string]: any }")
            .operator()
            .errors(&[(400, "BAD_REQUEST")]),
        get("/ops/sign-up/invites", "sign_up_invite_list", Ops, "Cluster sign-up invites")
            .ok("{ [string]: any }")
            .errors(&[(403, "FORBIDDEN")]),
        delete("/ops/sign-up/invites/{key}", "ops_sign_up_invite_revoke", Ops, "Withdraw a cluster sign-up invite")
            .proof_body()
            .ok("#Written")
            .operator(),
        post("/ops/sign-up/invites/{key}/revoke", "ops_sign_up_invite_revoke", Ops, "Withdraw a cluster sign-up invite (POST)")
            .about("For an HTTP client whose `DELETE` carries no body.")
            .proof_body()
            .ok("#Written")
            .operator(),
        post("/ops/outbound", "ops_outbound", Ops, "Approve outbound prefixes")
            .about("Where the operator holds outbound approvals.")
            .json::<super::OpsOutboundBody>("{ package: string, prefixes?: #OutboundSelection, replace?: bool, operatorProof?: #OperatorProof }")
            .ok("#Written")
            .operator(),
        post("/ops/apps/{app}/upgrade", "ops_upgrade", Ops, "Upgrade an app as the operator")
            .about("To a release the operator installed, where the operator holds app upgrades.")
            .json::<super::OpsUpgradeBody>("{ bundle?: string, package?: string, version?: string, service?: string, migrate?: string, operatorProof?: #OperatorProof }")
            .ok("{ index: uint, status: string, error: any, compute: bool }")
            .operator()
            .errors(&[(400, "AMBIGUOUS_COMPUTE"), (400, "BAD_REQUEST"), (409, "NOT_INSTALLED"), (409, "RELEASE_DEADLINE"), (422, "UPGRADE_FAILED")]),
        post("/ops/orgs/{org}/upgrade", "ops_org_upgrade", Ops, "An operator's upgrade run")
            .json::<super::OrgUpgradeBody>("{ package: string, version: string, team?: string, cascade?: bool, migrate?: string, confirm?: #Confirm, operatorProof?: #OperatorProof }")
            .ok("#UpgradeRun")
            .operator()
            .errors(&[(404, "NOT_FOUND")]),
        post("/ops/packages", "ops_package_upload", Ops, "Install an uploaded release as the operator")
            .raw("application/octet-stream", "the `.mpk`")
            .ok("{ package: string, version: string, [string]: any }")
            .errors(&[(400, "BAD_PACKAGE"), (400, "BAD_BUNDLE")]),
        get("/ops/packages", "ops_package_list", Ops, "The releases the operator installed").ok("{ packages: any[] }"),
        post("/ops/packages/install", "ops_package_install", Ops, "Install a release from the registry as the operator")
            .json::<super::InstallBody>("{ package: string, version: string }")
            .ok("{ package: string, version: string, [string]: any }"),
        delete("/ops/packages/{package}", "ops_package_uninstall", Ops, "Uninstall what the operator installed").ok("#Written"),
        delete("/ops/packages/{package}/{version}", "ops_package_version_uninstall", Ops, "Uninstall one operator-installed version").ok("#Written"),
        get("/ops/quotas", "ops_quotas_get", Ops, "The quotas")
            .about("The per-account quotas, and the per-payer overrides under `orgs`.")
            .ok("{ maxAppsPerAccount: uint, maxBundleBytesPerAccount: uint, maxApiKeysPerAccount: uint, maxOrgsPerAccount: uint, maxAccounts: uint, maxFileBytesPerOrg: uint, maxConcurrentRunsPerOrg: uint, computeFuelPerOrg: uint, maxFuelPerDeveloperPerDay: u64, orgs: any, [string]: any }"),
        post("/ops/quotas", "ops_quotas_set", Ops, "Change quotas")
            .about("Changes the limits the body names, the others keep their value; signed over `cluster / __set_quotas / { quotas }` with every limit after the change. With `org`: one payer's `maxConcurrentRuns`, `computeFuel` (decimal string) and `maxFileBytes` (`null` clears one). A name that is no quota is refused.")
            .value("{ org?: string, operatorProof?: #OperatorProof, [string]: any }")
            .ok("{}")
            .operator()
            .errors(&[(400, "BAD_REQUEST"), (400, "UNKNOWN_QUOTA"), (409, "UPGRADE_PENDING")]),
        get("/ops/pools", "ops_pools_get", Ops, "Compute pools")
            .about("The pools and the runs each takes.")
            .ok("{ [string]: any }"),
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
            .ok("#Written")
            .step_up(),
        post("/sign-up/invites", "sign_up_invite_create", AccountHolder, "Issue a cluster sign-up invite")
            .json::<super::SignUpInviteBody>("{ uses?: uint, ttlSecs?: uint, confirm?: #Confirm }")
            .ok("{ token: string, [string]: any }")
            .step_up()
            .errors(&[(400, "BAD_REQUEST"), (400, "WRONG_CHALLENGE")]),
        get("/sign-up/invites", "sign_up_invite_list", AccountHolder, "Cluster sign-up invites")
            .ok("{ [string]: any }")
            .errors(&[(403, "FORBIDDEN")]),
        delete("/sign-up/invites/{key}", "sign_up_invite_revoke", AccountHolder, "Withdraw a cluster sign-up invite")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("#Written")
            .step_up(),
        post("/deployers", "deployer_grant", AccountHolder, "Grant a deployer")
            .about("The cluster's owner, with a passkey.")
            .json::<super::DeployerBody>("{ deployer: #Deployer, until?: string, operatorProof?: #OperatorProof, confirm?: #Confirm }")
            .ok("{ key: string, deployer: #Deployer, grant: any }")
            .step_up(),
        get("/deployers", "deployer_list", AccountHolder, "The deployers")
            .ok("{ [string]: any }")
            .errors(&[(403, "FORBIDDEN")]),
        delete("/deployers/{key}", "deployer_revoke", AccountHolder, "Revoke a deployer")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("{ key: string }")
            .step_up(),
        post("/deployments", "deployment_create", AccountHolder, "Create a deployment")
            .about("A named set of releases an owner (an account or organization) offers for others to instantiate.")
            .json::<super::DeploymentBody>("{ name: string, owner: #Deployer, confirm?: #Confirm }")
            .ok("{ [string]: any }")
            .step_up()
            .errors(&[(403, "FORBIDDEN")]),
        get("/deployments", "deployment_list", SignedIn, "Deployments").ok("{ deployments: any[] }"),
        get("/deployments/{id}", "deployment_get", SignedIn, "One deployment")
            .ok("{ [string]: any }")
            .errors(&[(404, "NOT_FOUND")]),
        delete("/deployments/{id}", "deployment_delete", AccountHolder, "Delete a deployment")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("{ deployment: string }")
            .step_up(),
        post("/deployments/{id}/apps", "deployment_offer", AccountHolder, "Offer a release in a deployment")
            .json::<super::OfferBody>("{ app: string, package: string, version: string, confirm?: #Confirm }")
            .ok("{ deployment: string, app: string }")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        delete("/deployments/{id}/apps/{app}", "deployment_withdraw", AccountHolder, "Withdraw an offer")
            .opt_json::<super::ConfirmOnlyBody>("{ confirm?: #Confirm }")
            .ok("#Written")
            .step_up()
            .errors(&[(404, "NOT_FOUND")]),
        get("/deployments/{id}/orgs", "deployment_orgs", SignedIn, "The organizations a deployment serves")
            .ok("{ [string]: any }"),
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
        let name = o.handler.trim_start_matches("crate::").replace("::", "_");
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

fn error_response(status: u16, codes: &BTreeSet<&'static str>) -> Value {
    let details = match status {
        412 => Some("#/components/schemas/ConfirmRequired"),
        428 => Some("#/components/schemas/OperatorSignatureRequired"),
        _ => None,
    };
    let mut narrowed = json!({ "properties": { "error": { "enum": codes } } });
    if let Some(details) = details {
        narrowed["properties"]["details"] = json!({ "$ref": details });
    }
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
    json!({
        "description": described.join("; "),
        "content": { "application/json": { "schema": {
            "allOf": [{ "$ref": "#/components/schemas/Error" }, narrowed],
        } } },
    })
}

fn input_schema(input: &Input, shapes: &BTreeMap<&'static str, Shape>) -> Value {
    let mut schema = parse(input.shape).schema();
    if closed(input, shapes, false) {
        schema["additionalProperties"] = json!(false);
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
        description.push_str(
            " **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.",
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
        "summary": op.summary,
        "description": description,
        "security": op.who.security(),
        "x-calimero-who": op.who.text(),
        "x-calimero-sealed": !IN_THE_CLEAR.contains(&op.path),
    });
    if !params.is_empty() {
        out["parameters"] = Value::Array(params);
    }
    let body = match &op.body {
        Body::None => None,
        Body::Json(i) => Some((true, "application/json", input_schema(i, shapes), "")),
        Body::OptionalJson(i) | Body::RawJson(i) => Some((
            false,
            "application/json",
            input_schema(i, shapes),
            "May be absent.",
        )),
        Body::Raw(media, what) => Some((
            true,
            *media,
            json!({ "type": "string", "contentMediaType": media }),
            *what,
        )),
    };
    if let Some((required, media, schema, what)) = body {
        out["requestBody"] = json!({
            "required": required,
            "content": { media: { "schema": schema } },
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
        Reply::Bytes(what) => json!({
            "description": format!("OK: {what}."),
            "content": { "application/octet-stream": { "schema": { "type": "string", "contentMediaType": "application/octet-stream" } } },
        }),
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
            (c.name.to_owned(), schema)
        })
        .collect();
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Calimero node API",
            "version": "1",
            "description": DESCRIPTION,
            "license": { "name": "MIT OR Apache-2.0" },
        },
        "servers": [{
            "url": "https://{node}",
            "description": "A node's --listen address, reached through the sealed channel.",
            "variables": { "node": { "default": "localhost:2428" } },
        }],
        "tags": TAGS.iter().map(|(name, description)| json!({ "name": name, "description": description })).collect::<Vec<_>>(),
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
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use serde_json::{json, Value};

    use super::shape::parse;
    use super::{components, document, operations, shapes, Body, Input, Op};

    /// The sources the router's handlers live in, by the prefix `router` names them with.
    const SOURCES: &[(&str, &str)] = &[
        ("", include_str!("../api.rs")),
        ("origins::", include_str!("origins.rs")),
        ("account_proof::", include_str!("account_proof.rs")),
        ("crate::consent_page::", include_str!("../consent_page.rs")),
    ];

    /// `(method, path, handler)` of every `.route(..)` in `router`.
    fn routes() -> Vec<(String, String, String)> {
        let src = SOURCES[0].1;
        let start = src.find("pub fn router(").expect("router");
        let end = start + src[start..].find(".route_layer(").expect("route_layer");
        let table = &src[start..end];
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
                (Body::Raw(..) | Body::RawJson(_), None) => raw,
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
        let ops = operations();
        let inputs = ops.iter().flat_map(|op: &Op| {
            let body = match &op.body {
                Body::Json(i) | Body::OptionalJson(i) | Body::RawJson(i) => Some((i, false)),
                Body::None | Body::Raw(..) => None,
            };
            body.into_iter()
                .chain(op.query.iter().map(|q| (q, true)))
                .map(move |(i, query)| (op, i, query))
        });
        for (op, input, query) in inputs {
            let Some(ty) = input.ty else { continue };
            let at = format!("{} {} ({})", op.method, op.path, ty.name);
            let shape = parse(input.shape);
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
            wrong.is_empty(),
            "request shapes that differ from their types:\n{}",
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
