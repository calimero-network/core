# [product] short description

## Description

Please include a short description of the change and which issue is fixed.
Please also include relevant motivation and context. List any dependencies that
are required for this change.

## Test plan

Please describe the tests that you ran to verify your changes. Provide
instructions so we can reproduce. Is it possible to add a test case to our
end-to-end tests with changes from this PR? Add screenshots or videos for
changes in the user-interface.

## Proof the fix works

For a bug fix, show it actually works - not just that tests pass:

- **Reproduction**: the exact command / test / merobox scenario that triggers the bug.
- **Before**: the failing output or log line (the symptom).
- **After**: the same run now passing, plus the regression test that locks it in.

Skip this section only for pure docs/chore PRs.

## Wire contract (SDK gate)

If this PR changes an HTTP wire DTO or route, the SDKs mirror it by hand — keep
them in sync or the contract gate goes red:

- [ ] Regenerated wire fixtures (if a DTO changed):
      `UPDATE_FIXTURES=1 cargo test -p calimero-server-primitives --test wire_fixtures`
- [ ] Updated `crates/server/endpoints.json` (if routes changed):
      `UPDATE_MANIFEST=1 cargo test -p calimero-server --test route_manifest`
- [ ] Linked the matching mero-js PR — a breaking wire change needs a paired SDK update

To run the live SDK e2e against your paired SDK branch, add a line to this body
(defaults to `master`):

```
sdk-ref: <your-mero-js-branch>
```

## Trust boundary

Required when this PR touches a path in `TRUST_BOUNDARY_PATHS` in `scripts/check-trust-boundary.py`; CI fails if an item is missing or unanswered.
Tick exactly one box per line and keep the item text as is; "no" is the expected answer for items 2-4 and 6.
The rules: [AGENTS.md](https://github.com/calimero-network/core/blob/master/AGENTS.md#security-trust-boundaries).

- Adds input from peers, gossip, streams, HTTP callers or app guests: [ ] yes [ ] no
- Some new input has no named limit constant: [ ] yes [ ] no
- Takes an identity from a message field instead of a signature or the authenticated channel: [ ] yes [ ] no
- A gated operation is untested for kicked, left, deny-listed, revoked, descoped, inherited or other-namespace actors: [ ] yes [ ] no
- Changes a signed format, wire format or schema version: [ ] yes [ ] no
- Changes a signed or wire format without naming the paired SDK PR in `sdk-ref:`: [ ] yes [ ] no
- Changes a workflow's triggers, permissions, checkout or artifact handling: [ ] yes [ ] no

## Documentation update

Mention here what part (if any) of public or internal documentation should be
updated because of this PR. Documentation **has to be updated** no later than
**one day** after this PR has been merged.
