// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// Mero.js documentation — Astro Starlight with the shared Calimero theme
// (Zinc + #a5ff11 lime), ported from calimero-network/core.
export default defineConfig({
  site: 'https://calimero-network.github.io',
  // GitHub project Pages serve under /<repo>/. Change if a custom domain is used.
  base: '/mero-js',
  integrations: [
    starlight({
      title: 'Mero.js',
      description:
        'The JavaScript client for Calimero 1.0 — verify a node\'s TDX attestation, seal every request to it, sign in with passkeys or API keys, and run apps on their own replicated log.',
      logo: {
        light: './src/assets/logo-light.svg',
        dark: './src/assets/logo-dark.svg',
        alt: 'Mero.js',
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
        baseUrl: 'https://github.com/calimero-network/mero-js/edit/master/docs/',
      },
      head: [
        { tag: 'meta', attrs: { name: 'theme-color', content: '#09090b' } },
      ],
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/calimero-network/mero-js',
        },
      ],
      // Explicit, grouped navigation: Get Started → Understand → Guides → Reference.
      sidebar: [
        { label: 'Home', link: '/' },
        {
          label: 'Get Started',
          items: ['get-started/quickstart', 'get-started/authentication'],
        },
        {
          label: 'Understand',
          items: ['understand/system-overview', 'understand/glossary'],
        },
        {
          label: 'Guides',
          items: [
            'guides/apps',
            'guides/organizations',
            'guides/user-approvals',
            'guides/sealed-transport',
            'guides/bundling-and-runtimes',
          ],
        },
        {
          label: 'Reference',
          items: ['reference/client', 'reference/error-model', 'reference/migrating-from-v1'],
        },
      ],
    }),
  ],
});
