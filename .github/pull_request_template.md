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

## Documentation update

Mention here what part (if any) of public or internal documentation should be
updated because of this PR. Documentation **has to be updated** no later than
**one day** after this PR has been merged.
