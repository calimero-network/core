# calimero-outbound

Outbound HTTPS requests from a run, and the secrets they may carry: the
`OutboundHost` the node hands a run (`Node::outbound_for`), which the component
host's outgoing HTTP (`wasi:http`, `calimero_app_host::HttpHost`, `NodeHttp` in
`calimero-node`) calls with a borsh `calimero_primitives::outbound::OutboundRequest`.

A request is made only when every gate holds:

1. The operator turned it on (`OutboundSettings::enabled`) and allows the host
   (`hosts`, or `"*"` for any host an app declares).
2. The app's signed `.mpk` manifest declares a URL prefix covering it (`outbound`).
3. That prefix is approved for the app's publisher (`Approval`). In merod it
   is the app's organization (whoever holds `MANAGE_APPLICATION` org-wide, with
   a passkey assertion), or the operator where `--outbound-approvals operator`
   gives it the decision; every app is in an organization since 1.0.0-rc.20.
4. Egress is on for the app: neither its organization (`Org::egress_off`) nor the
   instance (`AppRecord::egress_off`) turned it off (`SystemCmd::SetEgress`, an owner or
   admin; on again only through the consent page). The node wraps every run's client in
   `egress_gated`, which asks before each request and refuses with `EGRESS_OFF`
   (`OutboundRefusal::EgressOff`) before any other gate, over every approval.
5. The run writes (calls, follow-ups, timers; a component's actions and, D76, its calls,
   timers and deliveries): never a view, `init`, a migration or compute (a compute run
   has no network at all, so the switch never concerns a job node).
6. `https` on 443 to a name resolving only to public addresses, size limits from
   `calimero_primitives::outbound`, no redirects; at most
   `calimero_app_host::MAX_HTTP_REQUESTS` (8) per run, counted by the host.

Secrets are **credential bindings**: the app never places, names or reads one.
A release's signed manifest binds each (`AppIdentity::secrets`, from `secrets`,
`calimero_bundle::BundleSecret`) to one host, path prefixes there (whole segments,
`applies_to` over the parsed URL's normalized path; `["/"]` for the whole host) and
one place: a header in a
`format` (`authorization`, `Bearer {}`) or a top-level body field of a
form-urlencoded or JSON body (Turnstile's `secret`). `prepare` attaches each binding
of the request's host and path itself (another path of the host gets none and goes
out as an ordinary request: nothing of the key rides on it) (`value_of`: `AppIdentity::own`, `OwnSecrets`, the
instance owner's else its deployment managers', `calimero_node::component::run_secrets`;
else `EnvSecrets` or a loopback `SidecarSecrets`, scoped by package, signer and a host
list that must hold the binding's host), and refuses a request that sets a bound
header (any case) or body field (however spelled: decoded form keys, parsed JSON
keys), or that writes `{{secret:` anywhere, URL (also percent-encoded), header or
body (`PLACEHOLDER_REFUSED`, naming `SECRETS_DOCS`). A body field is appended to a
form (`&field=value`) or spliced first into the JSON object, the app's bytes left as
they were and the value never in a `serde_json::Value`. No value: a `required`
binding refuses the request, another is left off. A release binding none sends
none. Why: a placeholder the app placed could go where an honest upstream reads it
as content and answers it back transformed (the reflection trick,
`an_app_cannot_read_a_secret_back_through_a_reflecting_upstream`), which no
redaction follows. The echo answers each attached value's length (`secretBytes`),
never a value.

What it logs (every line reaches the operator): a refusal as `refusal_kind`
(`NotApproved`, `AwaitingApproval`, `Invalid`) with the URL's host, a sent request
with the declared prefix that covers it, never the URL's path, query, body or a
name the guest wrote; the guest gets the refusal's words (`log_tests`).

A failed request reaches the guest and the `outbound request failed` log line (which
ships to the operator) only as its `FailureKind` (`timeout`, `connect`, `too large`,
`redirect`, `other`), never in the transport's words, which can quote the request.

Redaction (`src/redact.rs`) replaces each secret a request carried in the response
body, as sent and in the encodings an API commonly
echoes: hex (lower, upper), base64 (standard and URL-safe, padded or not, and the
part the secret alone decides inside a longer base64 value such as Basic
credentials), percent-encoding (RFC 3986 either case, `encodeURIComponent`, form)
and JSON string escapes (`serde_json`, Python's ASCII-only, PHP's `\/`). The
patterns are built once per request into one leftmost-longest Aho-Corasick scan;
responses are read whole, so a match never spans chunks. Bounds:
`MAX_SECRETS_PER_REQUEST` (32 distinct values, more is refused),
`PATTERNS_PER_SECRET` (20), encodings only for a secret of
`MIN_ENCODED_SECRET_BYTES` (8) to `MAX_ENCODED_SECRET_BYTES` (4096) bytes.
**Not covered**, so this is defence in depth, not a guarantee: any other
transformation (another or a nested encoding, mixed-case hex, compression,
encryption), a hash or MAC of a secret, and a partial echo (a prefix, a suffix, a
slice). Bindings keep the app from placing a secret where it is echoed; what the
key may do at its bound host is still the app's to use.

- `OutboundService::from_settings` → `for_run(app, declared, approval, context)`;
  `for_run_counted(.., sent)` also counts each request handed to the transport
  (the node's `calimero_app_outbound_requests_total`).
- `merge_approval` computes the approval an owner's selection leaves.
- `egress_gated(inner, EgressCheck)`: the egress switch, asked per request (so one
  turned off reaches a run already going); test
  `egress_off_refuses_every_request_whatever_was_approved_until_it_is_on_again`.
- `TURNSTILE_VERIFY_URL`: Cloudflare Turnstile's siteverify, for apps that check a
  token from a call (D76; `apps/fixture-captcha`) and for the `turnstile` stand-in.
- Feature `outbound-testing`: `[outbound.testing] echo` answers `.test` hosts
  in-process (cluster tests and scenarios) and takes literal secrets; `turnstile`
  answers `TURNSTILE_VERIFY_URL` in-process as Cloudflare answers its test secrets
  (`1x…AA` passes, `2x…AA` fails, `3x…AA` duplicate), spending each token once
  unless a check repeats its idempotency key.

```bash
cargo test -p calimero-outbound --features outbound-testing
```
