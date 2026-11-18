# mero-icons - agent guide

> mero-icons lives in calimero-network/core under `sdks/components/mero-icons`
> and ships with core: one version (core's root `VERSION`), one tag, one release.
> Read [sdks/AGENTS.md](../../AGENTS.md) (the SDK rules) and core's
> [RELEASING.md](../../../RELEASING.md) before committing or titling a PR. Never
> edit `version` in `package.json` by hand (`python3 scripts/sdks.py version-sync`).
> Its `CHANGELOG.md` is the history from the design-system repository; new
> entries go in core's release notes.

`@calimero-network/mero-icons` is the Calimero icon set: one React component per
icon (`forwardRef` to the `<svg>`, `size`, `strokeWidth`, `absoluteStrokeWidth`),
stroked in `currentColor`. ESM only (`exports` names `import`). mero-ui depends
on it, so a change here runs mero-ui's and mero-charts' CI too.

## Layout

- `src/`: one `.tsx` per icon, `IconBase.tsx`, and `index.ts` that exports them all.
- `raw/chunky.svg`: the source artwork.
- `tools/`: `scripts/build-icons.mjs` and `svgo.config.json`, from the design
  system's first icon pass (see below).
- `test/`: vitest; every exported icon renders an `<svg>` at its `size`.
- `sdk.json`: how core's CI treats this SDK (`scripts/sdks.py`).

## Build, test, lint

pnpm and its version come from `sdks/package.json`; the one lockfile is
`sdks/pnpm-lock.yaml`. Node 24 in CI. These are the steps
`.github/workflows/sdk-mero-icons.yml` runs:

```bash
cd sdks && pnpm install --frozen-lockfile
cd components/mero-icons
pnpm build                # tsup: dist/ (esm and cjs, with types)
pnpm test                 # vitest
pnpm typecheck
pnpm lint                 # eslint src test
```

## Generated code

None: `sdk.json` has no `generate`. The icons in `src/` are written and kept by
hand. `tools/scripts/build-icons.mjs` generated only the first three (`Clock`,
`ClockAlert`, `ClockX`) and rewrites `src/index.ts` with just those, so it is not
a generator to rerun: running it would drop every other export. Its
dependencies (`svgo`, `@svgr/core`, `globby`) are not installed. A new icon is a
new file in `src/` (on `IconBase`, like the others), exported from `index.ts`.
