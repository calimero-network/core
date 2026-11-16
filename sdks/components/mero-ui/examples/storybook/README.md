# mero-ui storybook

Every component of the Calimero design system: mero-ui, with mero-icons,
mero-tokens and mero-charts. The stories import each package's `src/` directly
(the aliases in `.storybook/main.ts`), so nothing needs building first.

```bash
cd sdks && pnpm install
pnpm --filter mero-ui-storybook dev              # http://localhost:6006
pnpm --filter mero-ui-storybook build-storybook  # storybook-static/
```

It is a private workspace package; it is never published.
