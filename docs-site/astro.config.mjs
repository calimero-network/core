// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// Phase 0 spike: prove Starlight + our dark theme + a mounted animated-SVG
// diagram (the bespoke engine as an Astro island). Content authored in MDX.
export default defineConfig({
  site: 'https://calimero-network.github.io',
  // GitHub project Pages serve under /<repo>/. Change if a custom domain is used.
  base: '/core',
  integrations: [
    starlight({
      title: 'Calimero Core',
      description:
        'Build, operate, and reimplement Calimero — a peer-to-peer framework for sandboxed WASM apps over causally-consistent shared state.',
      logo: { src: './src/assets/logo.svg', alt: 'Calimero Core' },
      favicon: '/favicon.svg',
      customCss: ['./src/styles/theme.css'],
      lastUpdated: true,
      editLink: {
        baseUrl: 'https://github.com/calimero-network/core/edit/master/docs-site/',
      },
      head: [
        { tag: 'meta', attrs: { name: 'theme-color', content: '#0d1117' } },
      ],
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/calimero-network/core',
        },
      ],
      // Explicit, grouped + sequenced navigation (not autogenerate): each track
      // separates "learn in order" from "look up", so the sidebar has a shape
      // instead of being one flat list. URLs are unchanged (grouping only).
      sidebar: [
        { label: 'Home', link: '/' },
        {
          label: 'Build',
          items: [
            { label: 'Overview', slug: 'build' },
            {
              label: 'Get started',
              items: ['build/quickstart', 'build/tutorial'],
            },
            {
              label: 'How-to guides',
              items: [
                'build/guides',
                'build/guides/collections',
                'build/guides/events',
                'build/guides/cross-context',
                'build/guides/blobs',
                'build/guides/access-control',
                'build/examples',
                'build/testing',
              ],
            },
            {
              label: 'Reference',
              items: ['build/sdk-macros', 'build/collections'],
            },
            {
              label: 'Deep dives',
              collapsed: true,
              items: [
                'build/permissioned-storage',
                'build/advanced-sdk',
                'build/storage-complexity',
                'build/gotchas',
              ],
            },
          ],
        },
        {
          label: 'Operate',
          items: [
            { label: 'Overview', slug: 'operate' },
            { label: 'Get started', items: ['operate/install'] },
            {
              label: 'Guides',
              items: [
                'operate/deployment',
                'operate/networking',
                'operate/security',
                'operate/observability',
                'operate/troubleshooting',
                'operate/runbooks',
              ],
            },
            {
              label: 'Reference',
              items: [
                'operate/merod',
                'operate/meroctl',
                'operate/config',
                'operate/admin-api',
                'operate/auth',
              ],
            },
          ],
        },
        {
          label: 'Protocol Reference',
          items: [
            { label: 'Overview', slug: 'protocol/overview' },
            {
              label: 'The core model',
              items: [
                'protocol/concepts',
                'protocol/identities',
                'protocol/operations',
                'protocol/projection',
              ],
            },
            {
              label: 'The planes',
              items: ['protocol/execution', 'protocol/governance'],
            },
            {
              label: 'Moving & reconciling',
              items: [
                'protocol/networking',
                'protocol/write-path',
                'protocol/receive-path',
                'protocol/sync',
              ],
            },
            {
              label: 'Subsystems',
              items: [
                'protocol/blobs',
                'protocol/tee-attestation',
                'protocol/xcall',
                'protocol/upgrades',
              ],
            },
            {
              label: 'Deep dives',
              collapsed: true,
              items: [
                'protocol/key-rotation',
                'protocol/divergence-recovery',
                'protocol/governance-edge-cases',
                'protocol/capability-inheritance',
                'protocol/crdt-internals',
                'protocol/encryption',
              ],
            },
            {
              label: 'Reference',
              items: ['protocol/glossary', 'protocol/storage'],
            },
          ],
        },
        {
          label: 'Contribute',
          items: [
            { label: 'Overview', slug: 'contribute' },
            { label: 'Orientation', items: ['contribute/architecture'] },
            {
              label: 'Working on core',
              items: ['contribute/development', 'contribute/docs'],
            },
            { label: 'Reference', items: ['contribute/adrs'] },
          ],
        },
      ],
    }),
  ],
});
