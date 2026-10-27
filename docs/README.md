# Calimero Docs

The Calimero Core documentation site, built with [Astro Starlight](https://starlight.astro.build/)
and published to <https://calimero-network.github.io/core/>.

- **Astro Starlight** shell — sidebar, built-in search, dark/light, responsive.
- The **Calimero dark theme** ported into Starlight tokens (`src/styles/theme.css`).
- **MDX authoring** — content as Markdown, diffs cleanly in PRs.
- A **bespoke animated-SVG diagram engine** mounted as an Astro island
  (`src/components/SeqDiagram.astro`), used across the tracks.

## Run it

```sh
cd docs
npm install
npm run dev      # http://localhost:4321/core/
npm run build    # static output in dist/
npm run check    # astro build + internal link check (what CI runs)
```

## Layout

Pages live in `src/content/docs/`, split by audience track — **Cloud** (release-v2
overview and quickstart), **Build**, **Operate**, **Protocol Reference**, and
**Contribute**. The sidebar is listed by hand in `astro.config.mjs`. See
`src/content/docs/contribute/docs.mdx` for the authoring guide.
