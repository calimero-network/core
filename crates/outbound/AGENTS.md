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
scoped by package, signer and host, and are redacted from the response.

- `OutboundService::from_settings` → `for_run(app, declared, approval, context)`;
  `for_run_counted(.., sent)` also counts each request handed to the transport
  (the node's `calimero_app_outbound_requests_total`).
- `merge_approval` computes the approval an owner's selection leaves.
- `TURNSTILE_VERIFY_URL`: Cloudflare Turnstile's siteverify, for apps that check a
  token from a call (D76; `apps/v2-fixtures/captcha`) and for the `turnstile` stand-in.
- Feature `outbound-testing`: `[outbound.testing] echo` answers `.test` hosts
  in-process (cluster tests and scenarios) and takes literal secrets; `turnstile`
  answers `TURNSTILE_VERIFY_URL` in-process as Cloudflare answers its test secrets
  (`1x…AA` passes, `2x…AA` fails, `3x…AA` duplicate), spending each token once
  unless a check repeats its idempotency key.

```bash
cargo test -p calimero-outbound --features outbound-testing
```
