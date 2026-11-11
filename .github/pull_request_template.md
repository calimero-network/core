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

- **Reproduction**: the exact command / test / scenario file that triggers the bug.
- **Before**: the failing output or log line (the symptom).
- **After**: the same run now passing, plus the regression test that locks it in.

Skip this section only for pure docs/chore PRs.

## Upgrade path

Every cluster rolls forward from the release in `.github/upgrade-from` (ci-checks.yml's
`Upgrade path`; docs: contribute/testing, "Upgrade path"):

- [ ] Adds user-visible functionality: added a deep upgrade case (`crates/node/tests/fault/upgrade_cases/`, `since` = the release it ships in) and regenerated the coverage matrix
- [ ] Changes stored data or a log entry: the upgrade path and `upgrade_replay` stay green (a reader or a migration), or the PR declares a hard cutover

## Wire contract (clients)

mero-js and calimero-client-py mirror the node's HTTP API, the sealed wire
format (`crates/sealed`) and the attestation check
(`crates/client/src/attest.rs`) by hand. `e2e.yml`'s `clients` job runs
both against this PR's `merod`:

- [ ] Changed a route, request or answer: updated both clients, or linked their PRs
- [ ] Changed the sealed format or the attestation check: changed mero-js in the same way

To test against a client branch other than a same-named one, add lines to this
body (both default to `master`):

```
sdk-ref: <your-mero-js-branch>
py-ref: <your-calimero-client-py-branch>
```

## Compatibility

What this PR does to earlier releases' data, peers, apps and clients
([docs/.../operate/compatibility.mdx](https://github.com/calimero-network/core/blob/master/docs/src/content/docs/operate/compatibility.mdx)).
A two-way door: anything it writes, the release before reads, and it removes
nothing earlier releases rely on. A one-way door names its reader, or declares
the hard cutover (with a "Hard cutover" line in CHANGELOG.md, a `[[cutover]]`
entry in `crates/node/api-deprecations.toml` for an API, WIT or ABI removal, and
mdma's `CUTOVERS` line). The `Compatibility door` check reads this block, and
requires it when the PR moves a format version.

**Door:** two-way

For a one-way door, add a line of one of these shapes:

- a `Reader:` line naming the migration step or decoder for the old shape, and the test that reads it;
- a `Hard cutover:` line saying what breaks, and for whom.

## Trust boundary

Required when this PR touches a path in `TRUST_BOUNDARY_PATHS` in `scripts/check-trust-boundary.py`; CI fails if an item is missing or unanswered.
Tick exactly one box per line and keep the item text as is; "no" is the expected answer for items 2-4 and 6.
The rules: [CODING_STANDARDS.md](https://github.com/calimero-network/core/blob/master/CODING_STANDARDS.md#security-trust-boundaries).

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
