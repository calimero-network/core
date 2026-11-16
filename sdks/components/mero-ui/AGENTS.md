# mero-ui - agent guide

> mero-ui lives in calimero-network/core under `sdks/components/mero-ui`
> and ships with core: one version (core's root `VERSION`), one tag, one release.
> Read [sdks/AGENTS.md](../../AGENTS.md) (the SDK rules) and core's
> [RELEASING.md](../../../RELEASING.md) before committing or titling a PR. Never
> edit `version` in `package.json` by hand (`python3 scripts/sdks.py version-sync`).
> Its `CHANGELOG.md` is the history from the design-system repository; new
> entries go in core's release notes.

`@calimero-network/mero-ui` is the Calimero design system's React component
library (React 19): layout, forms, data display, feedback, navigation, the
Tiptap `RichTextEditor`, and `styles.css` with the design tokens as CSS
variables. See [README.md](README.md); [DS_AUDIT.md](DS_AUDIT.md) is the audit
of the components and [docs/design-system-prompt.md](docs/design-system-prompt.md)
the original brief of the design system.

## Layout

- `src/`: one file per component, all exported from `src/index.ts`.
- `test/`: vitest smoke tests (server-rendered, no DOM).
- `examples/storybook/`: the storybook of every design-system component (see below).
- `tsup.config.ts`: the build; it also writes `dist/styles.css` from mero-tokens.
- `sdk.json`: how core's CI treats this SDK (`scripts/sdks.py`).

## The other design-system packages

- **mero-tokens** (`sdks/components/mero-tokens`) is a `workspace:*`
  *devDependency*: the build bundles it into `dist/` (it is not external) and
  turns its `cssVariables` into `dist/styles.css`. Typecheck resolves it to its
  `src/` (`paths` in `tsconfig.json`); tests and the build resolve its `dist/`.
- **mero-icons** (`sdks/components/mero-icons`) is a `workspace:*`
  *dependency*, kept external; pnpm publishes it as the exact version (VERSION).
- Tiptap is bundled (`noExternal`), so the published `dist/` imports no Tiptap
  package: two ProseMirror copies in a consumer break the editor.

A change to mero-icons or mero-tokens runs this package's CI too (`depends_on`
in `sdk.json`), and a change here runs mero-charts'.

## Build, test, lint

pnpm and its version come from `sdks/package.json`; the one lockfile is
`sdks/pnpm-lock.yaml`. Node 24 in CI. These are the steps
`.github/workflows/sdk-mero-ui.yml` runs:

```bash
cd sdks && pnpm install --frozen-lockfile
pnpm --filter @calimero-network/mero-tokens --filter @calimero-network/mero-icons build
cd components/mero-ui
pnpm build                # tsup: dist/ and dist/styles.css
pnpm test                 # vitest
pnpm typecheck
pnpm lint                 # eslint src test (warnings allowed)
pnpm build-storybook      # examples/storybook -> examples/storybook/storybook-static
```

## Generated code

None (`sdk.json` has no `generate`). Nothing here comes from core's OpenAPI
document.

## Examples

`examples/storybook/` is the private workspace package `mero-ui-storybook`
(Storybook 9, React 19): stories for every component of mero-ui, mero-icons,
mero-tokens and mero-charts, grouped as in the design system (Forms, Data
Display, Feedback, Charts, ...). Its `.storybook/main.ts` aliases the four
packages to their `src/`, so it needs no build first. `pnpm storybook` serves
it; `pnpm build-storybook` builds it (CI does). A new component gets a story
there.
