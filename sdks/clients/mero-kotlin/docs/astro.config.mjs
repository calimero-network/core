// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// mero-kotlin documentation — Astro Starlight with the shared Calimero theme
// (Zinc + #a5ff11 lime), ported from calimero-network/core.
export default defineConfig({
  site: 'https://calimero-network.github.io',
  // GitHub project Pages serve under /<repo>/. Change if a custom domain is used.
  base: '/kotlin-sdk',
  integrations: [
    starlight({
      title: 'mero-kotlin',
      description:
        'The Calimero Android SDK — a native Kotlin client for a remote Calimero node: coroutine-based auth with token refresh, JSON-RPC contract calls, the full admin API, live SSE events, SSO deep-link login, and a Jetpack Compose UI layer.',
      logo: {
        light: './src/assets/logo-light.svg',
        dark: './src/assets/logo-dark.svg',
        alt: 'mero-kotlin',
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
        baseUrl: 'https://github.com/calimero-network/kotlin-sdk/edit/master/docs/',
      },
      head: [
        { tag: 'meta', attrs: { name: 'theme-color', content: '#09090b' } },
      ],
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/calimero-network/kotlin-sdk',
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
            'guides/app-session',
            'guides/sealed-transport',
            'guides/agent-gate',
          ],
        },
        {
          label: 'Reference',
          items: ['reference/client', 'reference/error-model', 'reference/migrating-from-0x'],
        },
      ],
    }),
  ],
});
