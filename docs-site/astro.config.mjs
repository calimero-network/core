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
      // Vivid, high-contrast code highlighting with a code surface lifted off
      // the page background (theme-reactive via our gray vars) so blocks stand
      // out instead of blending into #0d1117.
      expressiveCode: {
        themes: ['github-dark', 'github-light'],
        styleOverrides: {
          borderRadius: '0.5rem',
          borderColor: 'var(--sl-color-gray-6)',
          codeBackground: 'var(--sl-color-gray-7)',
          codeFontFamily: 'var(--sl-font-mono)',
          frames: {
            editorTabBarBackground: 'var(--sl-color-gray-6)',
            terminalTitlebarBackground: 'var(--sl-color-gray-6)',
          },
        },
      },
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
          label: 'Start here',
          items: ['using-the-docs', 'journeys', 'topics'],
        },
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
              items: [
                'build/sdk-macros',
                'build/host-functions',
                'build/collections',
                'build/state-modeling',
                'build/error-handling',
              ],
            },
            {
              label: 'Deep dives',
              collapsed: true,
              items: [
                'build/permissioned-storage',
                'build/advanced-sdk',
                'build/migrations',
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
            // Ordered top-to-bottom by "what you most need to know first",
            // each part one level deeper than the last (see overview).
            { label: 'Overview', slug: 'protocol/overview' },
            {
              label: 'The replication loop',
              items: [
                'protocol/execution',
                'protocol/write-path',
                'protocol/receive-path',
              ],
            },
            {
              label: 'What gets replicated',
              items: [
                'protocol/operations',
                'protocol/projection',
                'protocol/data-anatomy',
              ],
            },
            {
              label: 'How it is organized',
              items: [
                'protocol/concepts',
                'protocol/governance',
                'protocol/capability-inheritance',
                'protocol/applications',
              ],
            },
            {
              label: 'Confidentiality & identity',
              items: [
                'protocol/identities',
                'protocol/encryption',
                'protocol/key-rotation',
                'protocol/security-model',
              ],
            },
            {
              label: 'Staying in sync',
              items: [
                'protocol/networking',
                'protocol/sync',
                'protocol/sync-internals',
                'protocol/divergence-recovery',
              ],
            },
            {
              label: 'Subsystems',
              items: [
                'protocol/blobs',
                'protocol/xcall',
                'protocol/tee-attestation',
                'protocol/upgrades',
              ],
            },
            {
              label: 'Deep internals & reference',
              collapsed: true,
              items: [
                'protocol/crdt-internals',
                'protocol/hlc',
                'protocol/storage',
                'protocol/limits',
                'protocol/governance-edge-cases',
                'protocol/glossary',
              ],
            },
          ],
        },
        {
          label: 'Contribute',
          items: [
            { label: 'Overview', slug: 'contribute' },
            {
              label: 'Orientation',
              items: ['contribute/architecture', 'contribute/crate-guide'],
            },
            {
              label: 'Working on core',
              items: ['contribute/development', 'contribute/testing', 'contribute/docs'],
            },
            { label: 'Reference', items: ['contribute/adrs'] },
          ],
        },
      ],
    }),
  ],
});
