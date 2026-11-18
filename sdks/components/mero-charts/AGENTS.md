# mero-charts - agent guide

> mero-charts lives in calimero-network/core under `sdks/components/mero-charts`
> and ships with core: one version (core's root `VERSION`), one tag, one release.
> Read [sdks/AGENTS.md](../../AGENTS.md) (the SDK rules) and core's
> [RELEASING.md](../../../RELEASING.md) before committing or titling a PR. Never
> edit `version` in `package.json` by hand (`python3 scripts/sdks.py version-sync`).
> Its `CHANGELOG.md` is the history from the design-system repository; new
> entries go in core's release notes.

`@calimero-network/mero-charts` holds the Calimero chart components on recharts:
`Gauge`, `TimeSeries` (with `calculateMean`, `calculateLast`, `calculateMax`),
`VectorTableRaw` and `TimeSeriesWithTable`. See [README.md](README.md).

## Layout

- `src/`: one file per chart, all exported from `src/index.ts`.
- `test/`: vitest smoke tests (server-rendered, no DOM).
- `sdk.json`: how core's CI treats this SDK (`scripts/sdks.py`).

## mero-ui: the workspace's own

`@calimero-network/mero-ui` is a `workspace:*` dependency (published as the
exact version, VERSION), so a change to mero-ui (or to mero-icons or mero-tokens
under it) runs this package's CI too (`depends_on` in `sdk.json`). recharts and
React stay external in the build.

## Build, test, lint

pnpm and its version come from `sdks/package.json`; the one lockfile is
`sdks/pnpm-lock.yaml`. Node 24 in CI. These are the steps
`.github/workflows/sdk-mero-charts.yml` runs:

```bash
cd sdks && pnpm install --frozen-lockfile
pnpm --filter @calimero-network/mero-tokens --filter @calimero-network/mero-icons build
pnpm --filter @calimero-network/mero-ui build
cd components/mero-charts
pnpm build                # tsup: dist/ (esm and cjs, with types)
pnpm test                 # vitest
pnpm typecheck
pnpm lint                 # eslint src test (warnings allowed)
```

The stories for these charts are in mero-ui's storybook
(`sdks/components/mero-ui/examples/storybook/src/Charts`).

## Generated code

None (`sdk.json` has no `generate`).
