# calimero-outbound

Outbound HTTPS requests from a run, and the secrets they may carry: the
`OutboundHost` the runtime's `http_request` host function calls. Ported from
v1's `calimero-context::outbound` so merod-v2 does not depend on the v1 context
crate.

A request is made only when every gate holds:

1. The operator turned it on (`OutboundSettings::enabled`) and allows the host
   (`hosts`, or `"*"` for any host an app declares).
2. The app's signed `.mpk` manifest declares a URL prefix covering it (`outbound`).
3. The owner approved that prefix for the app's publisher (`Approval`). In
   merod-v2 that is the organization owner (or the account, for an app outside
   an organization), with a passkey assertion; never the operator.
4. The run writes (calls, follow-ups, timers): never a view or a migration.
5. `https` on 443 to a name resolving only to public addresses, size limits from
   `calimero_primitives::outbound`, no redirects, one request per run.

Secrets (`{{secret:NAME}}`) come from `EnvSecrets` or a loopback `SidecarSecrets`,
scoped by package, signer and host, and are redacted from the response.

- `OutboundService::from_settings` → `for_run(app, declared, approval, context)`.
- `merge_approval` computes the approval an owner's selection leaves.
- Feature `outbound-testing`: `[outbound.testing] echo` answers `.test` hosts
  in-process (merobox, cluster tests) and takes literal secrets.

```bash
cargo test -p calimero-outbound --features outbound-testing
```
