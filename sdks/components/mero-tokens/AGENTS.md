# mero-tokens - agent guide

> mero-tokens lives in calimero-network/core under `sdks/components/mero-tokens`
> and ships with core: one version (core's root `VERSION`), one tag, one release.
> Read [sdks/AGENTS.md](../../AGENTS.md) (the SDK rules) and core's
> [RELEASING.md](../../../RELEASING.md) before committing or titling a PR. Never
> edit `version` in `package.json` by hand (`python3 scripts/sdks.py version-sync`).
> Its `CHANGELOG.md` is the history from the design-system repository; new
> entries go in core's release notes.

`@calimero-network/mero-tokens` holds the Calimero design tokens (colours,
spacing, radius, fonts) as data (`tokens.json`), as a typed object (`tokens`,
`colors`, `spacing`, `radius`, `fonts`) and as CSS variables (`cssVariables`, a
`:root { ... }` string). The package ships `dist/`, `src/` and `tokens.json`.
mero-design bundles it and writes its `styles.css` from `cssVariables`, so a change
here runs mero-design's and mero-charts' CI too.

## Layout

- `tokens.json`: the tokens, the single source of their values.
- `src/index.ts`: the exports, and the CSS variable list built from `tokens.json`.
- `test/`: vitest; every category exports and the CSS variables carry the values.
- `sdk.json`: how core's CI treats this SDK (`scripts/sdks.py`).

## Build, test, lint

pnpm and its version come from `sdks/package.json`; the one lockfile is
`sdks/pnpm-lock.yaml`. Node 24 in CI. These are the steps
`.github/workflows/sdk-mero-tokens.yml` runs:

```bash
cd sdks && pnpm install --frozen-lockfile
cd components/mero-tokens
pnpm build                # tsup: dist/ (esm and cjs, with types)
pnpm test                 # vitest
pnpm typecheck
pnpm lint                 # eslint src test
```

## Generated code

None (`sdk.json` has no `generate`). A new token goes in `tokens.json`, and in
`cssVariables` in `src/index.ts` when it should be a CSS variable.
