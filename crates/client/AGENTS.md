# calimero-client

Rust client for Calimero nodes: verifies the node's attestation
(`attest.rs`, shared with the node), seals every request (`calimero-sealed`),
signs in with a `Passkey` (feature `testing`: `SoftAuthenticator`) or an API key,
and calls every route of the node through a route layer generated from its OpenAPI
document (`client.orgs().get(..)`, `client.apps().call(..)`; below).

The attestation check (`attest::verify`) is the one place clients and peers decide
trust: quote genuine (or an allowed mock), report data =
`nonce || SHA-256("calimero.tee-attest.transport-key.v1" | app_hash | transport_key)`.
A real quote also needs a measurement policy (`Measurements`): `None` (the default)
refuses every real quote, `Pinned` checks fixed allowlists, `Release` fetches the
Sigstore-verified `published-mrtds.json` of the release the node names and requires a
matching allowed profile; then `policy_valid` (TCB status, debug TD, app-hash binding).
mero-js `src/cloud-v2/attestation.ts` implements the same check; change both or neither.
Node-only helpers beside it (no client or mero-js counterpart): `configured_min_release`
refuses a node's `--peer-min-release` below `DEFAULT_MIN_RELEASE`, and `check_own_profile` /
`verify_own_profile` check that a TD's own registers match its declared profile in its signed
release (`calimero_node::attest::check_own_profile`, at start on TDX).
The quote step itself (`verify_quote`, then `judge` for a real quote's registers) is
shared by `verify`, `verify_receipt_key` and `attest::release::verify_release`.
A document that carries `kms` (`KmsKeys`: what the node verified of the KMS its storage
and disk keys came from, asked with `purpose=kms`) passes `verify` only if its report
data is `kms_report_data`: `nonce || SHA-256("calimero.tee-attest.kms-evidence.v1" |
transport_binding | SHA-256(JCS(kms)))`. Opt in: a document without `kms` is checked
exactly as before. mero-js does not ask for it yet; the attestation verifier
(mero-tee) recomputes it, pinned by `the_kms_binding_matches_the_published_vector`.

The receipt-key binding hashes `Governance::canonical`, the governance JSON in
declaration order; `deployments` and `deployers` are written only when not the
defaults (`open`, `operator`), so a default cluster binds the bytes it did before
(`the_default_governance_binds_the_bytes_it_did_before_deployers`). A client that
recomputes it must keep them when the attested object has them; mero-js does
not compute this binding yet.

`verify_job_key` (`purpose=jobs`, a job node's attestation, `AttestationDoc::job_key`,
absent from every other document) shares `verify_report`: report data =
`nonce || SHA-256("calimero.tee-attest.job-key.v1" || x25519 transport key || ed25519 job key || release || cluster || node id)`
(text length-prefixed), so swapping any of them breaks it. It answers `(transport_key, job_key)`; only nodes use it
(the leader checking a job node), mero-js needs nothing.

`verify_orchestrator` (`purpose=orchestrator`, any replica of a cluster; `AttestationDoc::cluster_identity`) shares `verify_report`: report data =
`nonce || SHA-256("calimero.tee-attest.orchestrator.v1" || x25519 transport key || ed25519 cluster identity)`
(`orchestrator_report_data`, one function for node and client), so a quote of another purpose never verifies as it.
The caller passes the identity it expects (`GET /ops/cluster`) and gets the transport key back; a job node answers `404`.
mero-js has no counterpart yet; the vector is `the_orchestrator_binding_matches_the_published_vector`.

Release attestations (`attest/release.rs`, made by `merod attest`): a quote a node makes
before it holds any key, report data
`nonce || SHA-256("calimero.tee-attest.release.v1" || u32-BE-length-prefixed merodVersion,
merodSha256, teeRelease, teeProfile, identity)`. `verify_release` refuses another format,
nonce or claim, then runs the same quote check (a `Release` policy reads the release the
claim names) and refuses an envelope whose registers or profile are not the verified
quote's. `fetch_release_attestation` gets one from `GET /attestation/release`. Only
managers use it (mdma, `meroctl attest-release`); mero-js has no counterpart.

Errors: `Error::Api` is the node's answer (`{ error, message, details }`);
`Error::Transport` is not reaching, attesting or understanding it. One envelope
refusal is an answer too: a sealed request past `MAX_SEALED_BYTES` comes back
`413` in the clear, and `sealed_error` makes it `Error::Api { 413, TOO_LARGE }`.
`421 WRONG_NODE` (the node runs a release the account's owners have not
approved: it neither issues nor takes their sessions) is `Error::WrongNode`: send
the request to another node.

Sessions refresh on their own: a `401` to a request sent with a session token this
client holds the refresh token of is refreshed once (`Client::refresh`, which any
node takes) and the request sent once more (`send_payload`; bodies are kept as a
`Payload` for that). Requests that get a `401` together refresh once
(`refreshing`): a refresh token presented twice ends the session. A `401` to
`auth/refresh` itself, an API key, or a session held without its refresh token is
returned as it is. Why a `401` can need it: the node that issued the session was
removed, or replaced on a new data directory (a new signing key); a node's
restart keeps its key, and a stopped node's sessions verify on the others
(node `AGENTS.md`, "Sessions are node-signed"). Test:
`a_removed_nodes_sessions_are_refused` (node `tests/cluster.rs`).

Calls are retried with the same key (D70): `client.apps().call` (and
`apps().method`, the routes the document marks `x-calimero-retry-requires:
idempotencyKey`) always sends an idempotency key (the caller's, else a new
`node:c-…` key, which the node keeps in its short-lived class, 10 minutes) and
sends the call again with that key while the node answers `503` or cannot be
reached (`Error::is_unavailable`; 200 ms doubling to 2 s, 6 retries); `apps().view`
is retried too. A call that did commit is answered from its record. A call sent
with a passkey (a `confirm` method) is not retried: its challenge is single-use. Tests: `retry_tests`, and `calls_without_a_key_each_run_once` in
`crates/node/tests/cluster.rs`.

Files are uploaded by `Client::upload_file(app, reader, size)` (`files.rs`, `meroctl file upload`):
a file of at most one part (`FILE_PART_BYTES`, 16 MiB) is one `POST /apps/{app}/files`; a larger one
begins a session (`POST .../files/uploads`) and sends each part with `PUT .../parts/{k}`, reading one
part at a time from a seekable reader. A part that gets a `503` or a dropped connection is sent
again, then the node's `next` (`GET .../uploads/{u}`) is asked for; `409 PART_OUT_OF_ORDER` moves to
the `next` in its `details`; `404 UPLOAD_UNKNOWN` begins the session again once; `complete` is
retried on `503` with backoff; any other refusal (`413`, `429`) aborts the session and is returned.
Tests: `files::tests`, against a mock node over the real sealed channel, with a proxy that cuts
the connection.

`Client::export_file(app, id)` (`files.rs`, `meroctl file download`, `meroctl pipeline download`) returns a sealed file's plaintext: it makes a fresh X25519 key pair, asks `POST .../files/{id}/export`, unwraps the data key with `calimero_sealed::wrap::unwrap_export`, fetches each segment from `GET .../segments/{k}`, and checks the segment list against the file id, each segment against its hash and the size and SHA-256 of the plaintext before returning any byte.

`receipt.rs` holds the signed pipeline run receipt (`PipelineReceipt`, `StepRunProof`) and the one message the node signs and `verify_pipeline_receipt` rebuilds: `"calimero.pipeline.receipt.v1\0" || JCS(receipt)`, checked over the JSON as sent. `Client::pipeline_receipt` fetches it and `Client::verify_receipt` checks it under the answering node's key from `verify_app_cluster`; `meroctl pipeline receipt --verify` also compares each downloaded output with the receipt. A job's own report signature is not verified here yet.

`pipeline_events.rs` holds the typed, metadata-only events of `GET /apps/{app}/pipeline-runs/{run}/events` (`RunEvent`, `StepEvent`, `Snapshot`, `PipelineEvent`) and `RunProgress`, which reads the run view's JSON into only those fields and diffs two views; the node sends them and `Client::pipeline_run_events` (`meroctl pipeline watch`) reads them. A key set test pins each event's fields, so a new record field never reaches a stream.

Runtime and routes (`transport.rs`): `Transport` is everything that is not a
route: the attested, sealed channel (`SealedTransport::attested`), the
credential and the `401` refresh above, error mapping (`answer`, `api_error`,
`sealed_error`), the two-round approvals (`Transport::approved`: a `412
CONFIRM_REQUIRED` answered with a passkey's assertion, `confirm`; a `428
OPERATOR_SIGNATURE_REQUIRED` with the operator's signature, `operatorProof`;
`confirmed` is the passkey-only form) and SSE decoding (`Transport::events`). Its
route-facing surface is `send(Request) -> reqwest::Response` (any status),
`json(Request)`, `approved(Request, passkey, operator)` and `events(Request)`,
where `Request` is method, path, query, headers, `Body` (`Empty`, `Json`, `Bytes`)
and `Credential` (`Held`, the transport's own and refreshed; `Authorization`, a
caller's value passed as given; `Anonymous`). `meroctl proxy` forwards plain HTTP
through `Transport::send`. mero-js (`src/runtime/`) and calimero-client-py
(`calimero/transport.py`) have the same split.

The route layer is generated from the node's OpenAPI document
(`docs/public/openapi.json`) by `calimero-client-gen` (`tools/client-gen`), as
mero-js (`scripts/gen-methods.mjs`), calimero-client-py
(`scripts/generate_openapi_client.py`) and the Kotlin SDK (`codegen/facade.py`)
generate theirs, and checked in under `src/generated/` (never edited by hand):

- `routes.rs`: one async method per operation on its `x-calimero-group`'s
  accessor, `client.public()`, `auth()`, `orgs()`, `apps()`, `packages()`,
  `consent()`, `deployments()`, `ops()`. Names are the other SDKs': the
  operationId in snake case without a first word that repeats the group
  (`org_get` is `orgs().get`), unless the rest is a keyword (`app_move`) or
  another method's name; `intents` is `apps().call`, `views` is `apps().view`.
  Arguments are the path parameters, then the JSON body's fields and the query
  (required first), flat; a body that would make more than six arguments, is not
  one plain object, is open, or shares a parameter's name is taken whole as its
  type (`apps().create(AppCreateRequest { .. })`). A raw body is `bytes` (and
  `content_type` for `*/*`), a binary answer `Vec<u8>`, a `text/plain` one a
  `String`. The answer is the route's type; a refusal is `Error`.
- From the extensions: `x-calimero-step-up` adds `passkey: Option<&mut dyn
  Passkey>` (`None` sends as is and returns the `412`; `confirm` is never an
  argument), `x-calimero-operator` adds `operator: Option<&OperatorSigner>`
  (`operatorProof` likewise), `x-calimero-retry-safe` resends while
  `Error::is_unavailable` (`retry_unavailable`, D70) unless approving, with the
  `x-calimero-retry-requires` key made when none is given (`new_call_key`), and
  `x-calimero-sealed: false` (`/health`, `/attestation`) goes in the clear
  (`route::clear`). Event streams are not methods: `Client::events`,
  `Client::presence_stream`.
- `types.rs`: components keep their names; inline schemas are named after where
  they are (`{Operation}Request`, `{Operation}Response`, then each property).
  Absent is `Option<T>`; absent or `null` is `Option<Option<T>>` (and an optional
  `Value` keeps a sent `null`), both through `route::present`; an answer that
  is only `null` (`type: null`) is `()`. A struct an answer
  reaches keeps unnamed fields in `extra`, so re-serializing loses nothing; a
  struct only requests carry whose schema is closed (`additionalProperties:
  false`, and no answer or `x-calimero-error-details` reaches it) is
  `#[serde(deny_unknown_fields)]`, so a body read from a caller's JSON
  (`meroctl ops set-quotas`) refuses a misspelled key as the node would; a
  string enum keeps an unknown value in `Other(String)`. `Assertion`,
  `Registration`, `Session`, `AttestationDoc`, `Governance`, `Receipt` and
  `Release` are the client's own types (`pub use`), so what `attest` checks is
  what the routes answer. `Error` is not generated: every refusal is `Error`.

The hand-written flows are built on the generated methods: `sign_up*`,
`sign_in*`, `add_passkey`, `start_recovery` (passkey and recovery ceremonies),
`verify_app_cluster` (attestation), `upload_file` (`files.rs`), and the event
streams. The route methods written by hand before (`Client::get`, `post`,
`call`, `create_app`, ...) are `#[deprecated]`, each naming its replacement, and
go once `crates/node/tests` and `crates/merod/tests` stop calling them.

A route change regenerates the document and then this layer, in the same change:

```bash
UPDATE_OPENAPI=1 cargo test -p calimero-node --lib openapi
cargo run -p calimero-client-gen
```

`the_checked_in_client_is_what_the_document_generates` (in `tools/client-gen`,
run by CI's workspace tests) regenerates into a temporary directory and fails
while the checked-in code differs. Tests of the generated layer against a node:
`crates/node/tests/generated_routes.rs` (a step-up route, an operator route, raw
bodies, an explicit `null`).

The command line over this client is `meroctl` (`crates/meroctl`).

```bash
cargo test -p calimero-client
cargo test -p calimero-client-gen      # the generated layer is current
cargo test -p calimero-node --test generated_routes
```
