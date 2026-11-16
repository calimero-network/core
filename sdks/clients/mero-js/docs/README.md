# Mero.js Docs

The Mero.js documentation site, built with [Astro Starlight](https://starlight.astro.build/)
and published to <https://calimero-network.github.io/mero-js/>. Theme, favicon,
and the shared Calimero look (including the animated sequence-diagram engine) are
ported from [calimero-network/core](https://github.com/calimero-network/core/tree/master/docs).

## Run it

```sh
cd docs
npm install
npm run dev      # http://localhost:4321/mero-js/
npm run build    # static output in dist/
npm run check    # astro build + internal link check (what CI runs)
```

## Layout

Pages live in `src/content/docs/`, grouped into four tracks:

- **Get Started** — install, sign in with a passkey, and make your first call.
- **Understand** — how a request reaches an app, and the glossary.
- **Guides** — apps and packages, organizations and approvals, trust and the
  sealed channel, bundling and runtimes.
- **Reference** — `CalimeroClient`, the error model, and migrating from v1.

## Diagrams

Flow pages use the shared animated sequence-diagram engine:

- `src/components/SeqDiagram.astro` — data-driven diagrams authored inline in MDX.
- `src/components/Figure.astro` — wraps a hand-authored SVG in the same animation/Replay shell.
- `src/scripts/diagrams.client.ts` — the client engine (renders + wires Replay).

The engine, theme (`src/styles/theme.css`), and the base-prefix middleware
(`src/middleware.ts`) are shared verbatim with the other Calimero docs sites.
