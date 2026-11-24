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
4. The run writes (calls, follow-ups, timers; a component's actions and, D76, its calls,
   timers and deliveries): never a view, `init`, a migration or compute.
5. `https` on 443 to a name resolving only to public addresses, size limits from
   `calimero_primitives::outbound`, no redirects; at most
   `calimero_app_host::MAX_HTTP_REQUESTS` (8) per run, counted by the host.

Secrets (`{{secret:NAME}}`) come from `EnvSecrets` or a loopback `SidecarSecrets`,
scoped by package, signer and host, and are redacted from the response. A release
that declares its secrets (`AppIdentity::secrets`, from its signed manifest's
`secrets`) gets only those names, each only into a request to one of its declared
hosts; `None` keeps the operator's bindings alone. A value set in merod for a
declared name (`AppIdentity::own`, `OwnSecrets`: the instance owner's, else its
deployment managers'; `calimero_node::component::run_secrets`) wins over the
operator's for that name, still only to its declared hosts. The echo answers each
filled-in value's length (`secretBytes`), never a value.

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
slice). A secret should go only to a host that does not echo it.

- `OutboundService::from_settings` → `for_run(app, declared, approval, context)`;
  `for_run_counted(.., sent, audit)` also counts each request handed to the transport
  (the node's `calimero_app_outbound_requests_total`).
- `merge_approval` computes the approval an owner's selection leaves.
- `OutboundAudit` (`audit.rs`): each request a `for_run_counted` client is handed,
  sent or refused, as an `AuditEntry` under the run's `context` (the instance): time,
  method, host, path (no query, `MAX_AUDIT_PATH_BYTES`), status or `AuditFailure`
  (`SecretRefused` marks a secret refusal), the secret names (`secret_names`, the one
  reader of a request's placeholders), body sizes. Never a value, body, query or
  header. Bounded per node: `MAX_AUDIT_ENTRIES`, `AUDIT_RETENTION_MS`,
  `MAX_AUDIT_INSTANCES`. The node owns one (`Node::outbound_audit`).
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
