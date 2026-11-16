## Description

<!-- What changed and why. Link the issue if any. -->

## Core compatibility

mero-js mirrors core 1.0's node API (`crates/node/src/api.rs`) and its sealed
transport (`crates/sealed`) by hand. If this PR changes a route, a body or the
wire format:

- [ ] `pnpm test:cluster` passes against a merod built from the matching core branch
- [ ] Linked the matching core PR. Core's `e2e.yml` (`clients`) runs this suite
      against core's tree; name this branch like the core one, or put
      `sdk-ref: <this branch>` in the core PR's body, to pair them.

## Test plan

- [ ] `pnpm test:unit`
- [ ] `pnpm typecheck`, `pnpm typecheck:consumer` and `pnpm typecheck:tests`
- [ ] `pnpm lint`
- [ ] `pnpm build`
