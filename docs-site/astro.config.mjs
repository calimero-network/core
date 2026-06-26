// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// Phase 0 spike: prove Starlight + our dark theme + a mounted animated-SVG
// diagram (the bespoke engine as an Astro island). Content authored in MDX.
export default defineConfig({
  site: 'https://calimero-network.github.io',
  integrations: [
    starlight({
      title: 'Calimero Core',
      description:
        'Build, operate, and reimplement Calimero — a peer-to-peer framework for sandboxed WASM apps over causally-consistent shared state.',
      customCss: ['./src/styles/theme.css'],
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/calimero-network/core',
        },
      ],
      sidebar: [
        {
          label: 'Start here',
          items: [{ label: 'Overview', slug: 'index' }],
        },
        {
          label: 'Protocol Reference',
          autogenerate: { directory: 'protocol' },
        },
      ],
    }),
  ],
});
