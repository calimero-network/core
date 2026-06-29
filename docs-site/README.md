# Calimero Docs — Astro Starlight spike (Phase 0)

Proof-of-concept for the documentation overhaul (see
[`../docs/documentation-roadmap.md`](../docs/documentation-roadmap.md)).

It demonstrates, on real tooling:

- **Astro Starlight** shell — sidebar, built-in search, dark/light, responsive.
- The **Calimero dark theme** ported into Starlight tokens (`src/styles/theme.css`).
- **MDX authoring** — content as Markdown, diffs cleanly in PRs.
- The **bespoke animated-SVG diagram engine** mounted as an Astro island
  (`src/components/SeqDiagram.astro`), used on the Protocol Overview page.

This site lives **outside `architecture/`** on purpose: the doc-update bot's
`static_docs_dirs` is `architecture/` only, so authored content here is fenced
from it by construction.

## Run it

```sh
cd docs-site
npm install
npm run dev      # http://localhost:4321
npm run build    # static output in dist/
```

## What's here vs. next

This is the **walking skeleton**: the landing page (audience-routed) and one
ported Protocol chapter (Overview) with a live animated diagram. The full
content migration — porting the ten protocol chapters + appendix, then the
Build / Operate / Contribute tracks — happens in the later roadmap phases.
