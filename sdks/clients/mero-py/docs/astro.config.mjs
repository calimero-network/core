// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// calimero-client-py documentation — Astro Starlight with the shared Calimero
// theme (Zinc + #a5ff11 lime), ported from calimero-network/core.
export default defineConfig({
  site: 'https://calimero-network.github.io',
  // GitHub project Pages serve under /<repo>/. Change if a custom domain is used.
  base: '/calimero-client-py',
  // Keep the SeqDiagram rendering engine as an external chunk (not inlined),
  // which the diagrams client script loads at runtime — required for it to work.
  vite: { build: { assetsInlineLimit: 0 } },
  integrations: [
    starlight({
      title: 'Calimero Python Client',
      description:
        'The Python client for Calimero 1.0 — verify a node\'s attestation, sign in, deploy apps, call and view them, and administer organizations and teams. Native Rust core via PyO3.',
      logo: {
        light: './src/assets/logo-light.svg',
        dark: './src/assets/logo-dark.svg',
        alt: 'Calimero Python Client',
      },
      favicon: '/favicon.svg',
      customCss: ['./src/styles/theme.css'],
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
        baseUrl:
          'https://github.com/calimero-network/calimero-client-py/edit/master/docs/',
      },
      head: [
        { tag: 'meta', attrs: { name: 'theme-color', content: '#09090b' } },
      ],
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/calimero-network/calimero-client-py',
        },
      ],
      // Explicit, grouped navigation: Get Started → Reference.
      sidebar: [
        { label: 'Home', link: '/' },
        {
          label: 'Get Started',
          items: [
            'get-started/installation',
            'get-started/quickstart',
            'get-started/concepts',
          ],
        },
        {
          label: 'Reference',
          items: ['reference/client', 'reference/methods', 'reference/errors'],
        },
        {
          label: 'Guides',
          items: ['guides/migration'],
        },
      ],
    }),
  ],
});
