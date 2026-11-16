# mero-kotlin Docs

The mero-kotlin (Calimero Android SDK) documentation site, built with
[Astro Starlight](https://starlight.astro.build/) and published to
<https://calimero-network.github.io/kotlin-sdk/>. Theme, favicon, and the shared
Calimero look (including the animated sequence-diagram engine) are ported from
[calimero-network/core](https://github.com/calimero-network/core/tree/master/docs).

## Run it

```sh
cd docs
npm install
npm run dev      # http://localhost:4321/kotlin-sdk/
npm run build    # static output in dist/
npm run check    # astro build + internal link check (what CI runs)
```

## Layout

Pages live in `src/content/docs/`, grouped into four tracks:

- **Get Started** — install (Gradle/Maven), authenticate, and make your first call.
- **Understand** — the layered architecture, the token/refresh model, and the glossary.
- **Guides** — contexts & apps, executing methods, live events, groups & governance,
  blobs, and the Jetpack Compose frontend (`mero-compose`).
- **Reference** — the `Mero` class, the full admin & auth API surface, and the error model.

> The docs describe the Kotlin API; the SDK itself lives under `mero-core`
> (`com.calimero.mero`) and the optional Compose UI under `mero-compose`.

## Diagrams

Flow pages use the shared animated sequence-diagram engine:

- `src/components/SeqDiagram.astro` — data-driven diagrams authored inline in MDX.
- `src/components/Figure.astro` — wraps a hand-authored SVG in the same animation/Replay shell.
- `src/scripts/diagrams.client.ts` — the client engine (renders + wires Replay).

The engine, theme (`src/styles/theme.css`), and the base-prefix middleware
(`src/middleware.ts`) are shared verbatim with the other Calimero docs sites.
