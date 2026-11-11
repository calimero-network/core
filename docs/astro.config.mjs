// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// The docs site: Starlight, the landing's theme (src/styles/theme.css), and hand-authored SVG diagrams
// (src/components/diagrams) plus the SeqDiagram engine. Content is MDX.
export default defineConfig({
  site: 'https://calimero-network.github.io',
  // GitHub project Pages serve under /<repo>/. Change if a custom domain is used.
  base: '/core',
  // Deleted pages: old links land on the page that now covers the subject.
  redirects: {
    '/protocol/operations/': '/core/protocol/replication/',
    '/protocol/projection/': '/core/protocol/replication/',
    '/protocol/sync/': '/core/protocol/replication/',
    '/protocol/sync-internals/': '/core/protocol/replication/',
    '/protocol/receive-path/': '/core/protocol/replication/',
    '/protocol/divergence-recovery/': '/core/protocol/replication/',
    '/protocol/data-anatomy/': '/core/protocol/storage/',
    '/protocol/hlc/': '/core/protocol/storage/',
    '/protocol/crdt-internals/': '/core/protocol/storage/',
    '/protocol/capability-inheritance/': '/core/protocol/organizations/',
    '/protocol/governance-edge-cases/': '/core/protocol/organizations/',
    '/protocol/accounts/': '/core/protocol/identities/',
    '/protocol/delegated-authorship/': '/core/protocol/authorship/',
    '/protocol/direct-admission/': '/core/protocol/authorship/',
    '/protocol/tee-authorship/': '/core/protocol/authorship/',
    '/protocol/client-paths/': '/core/protocol/write-path/',
    '/cloud/operate/': '/core/operate/deployment/',
    '/cloud/coming-from-v1/': '/core/protocol/glossary/',
    '/build/delegated-execution-client/': '/core/protocol/write-path/',
    '/build/cargo-mero/': '/core/build/packaging-signing/',
    // SDK 0.12 pages, folded into the component SDK's reference.
    '/build/abi/': '/core/build/app-abi/',
    '/build/advanced-sdk/': '/core/build/sdk-macros/',
    '/build/state-modeling/': '/core/build/collections/',
    '/build/choosing-state/': '/core/build/collections/',
    '/build/storage-complexity/': '/core/build/collections/',
    '/build/permissioned-storage/': '/core/build/guides/access-control/',
    '/build/guides/securing-state/': '/core/build/guides/access-control/',
    '/build/guides/collections/': '/core/build/collections/',
  },
  integrations: [
    starlight({
      title: 'Calimero Core',
      description:
        'Build, operate, and understand Calimero — a confidential cloud for collaborative WASM apps: each app replicated by its own Raft log across attested TDX nodes.',
      logo: {
        light: './src/assets/logo-light.svg',
        dark: './src/assets/logo-dark.svg',
        alt: 'Calimero Core',
        replacesTitle: true,
      },
      favicon: '/favicon.svg',
      customCss: ['./src/styles/theme.css'],
      // Code blocks in the landing's style: square, ruled, on the paper-2
      // surface (theme-reactive via the gray vars theme.css maps).
      expressiveCode: {
        themes: ['github-dark', 'github-light'],
        styleOverrides: {
          borderRadius: '0',
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
        baseUrl: 'https://github.com/calimero-network/core/edit/master/docs/',
      },
      head: [
        { tag: 'meta', attrs: { name: 'theme-color', content: '#121211' } },
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
          // Overview, quickstart and writing apps.
          label: 'Get started',
          items: ['cloud/overview', 'cloud/quickstart', 'cloud/apps', 'cloud/deploy-from-ci'],
        },
        {
          label: 'Build',
          items: [
            { label: 'Overview', slug: 'build' },
            {
              label: 'Get started',
              items: ['build/quickstart', 'build/tutorial', 'build/ship-v2'],
            },
            {
              label: 'How-to guides',
              items: [
                'build/guides',
                'build/guides/collaborative-text',
                'build/guides/events',
                'build/guides/cross-context',
                'build/guides/blobs',
                'build/guides/search',
                'build/guides/outbound',
                'build/guides/compute-actions',
                'build/guides/access-control',
                'build/packaging-signing',
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
                'build/app-abi',
                'build/error-handling',
              ],
            },
            {
              label: 'Deep dives',
              collapsed: true,
              items: [
                'build/migrations',
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
                'operate/plain-http',
              ],
            },
            {
              label: 'Reference',
              items: [
                'operate/merod',
                'operate/meroctl',
                'operate/config',
                { label: 'Node API', slug: 'operate/admin-api' },
                { label: 'Node API (OpenAPI)', slug: 'operate/api-reference' },
                { label: 'Authentication', slug: 'operate/auth' },
                'operate/compatibility',
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
              label: 'The model',
              items: ['protocol/concepts', 'protocol/system-group', 'protocol/replication'],
            },
            {
              label: 'The call path',
              items: ['protocol/execution', 'protocol/write-path', 'protocol/reads'],
            },
            {
              label: 'Trust',
              items: [
                'protocol/networking',
                'protocol/tee-attestation',
                'protocol/encryption',
                'protocol/key-rotation',
                'protocol/security-model',
              ],
            },
            {
              label: 'People & authority',
              items: [
                'protocol/identities',
                'protocol/authorship',
                'protocol/organizations',
                'protocol/governance',
                'protocol/consent-page',
                'protocol/operator',
              ],
            },
            {
              label: 'Apps',
              items: [
                'protocol/applications',
                'protocol/context-lifecycle',
                'protocol/upgrades',
                'protocol/xcall',
                'protocol/blobs',
                'protocol/compute-actions',
                'protocol/presence',
              ],
            },
            {
              label: 'Reference',
              collapsed: true,
              items: [
                'protocol/storage',
                'protocol/limits',
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
          ],
        },
      ],
    }),
  ],
});
