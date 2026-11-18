// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// MeroKit documentation — Astro Starlight with the shared Calimero theme
// (Zinc + #a5ff11 lime), ported from calimero-network/core.
export default defineConfig({
  site: 'https://calimero-network.github.io',
  // GitHub project Pages serve under /<repo>/. Change if a custom domain is used.
  base: '/swift-sdk',
  integrations: [
    starlight({
      title: 'MeroKit',
      description:
        'The Calimero Swift SDK for Calimero 1.0: attested, sealed requests to a confidential cluster, passkey sign-in, app calls and events, orgs and teams, and a SwiftUI app layer.',
      logo: {
        light: './src/assets/logo-light.svg',
        dark: './src/assets/logo-dark.svg',
        alt: 'MeroKit',
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
        baseUrl: 'https://github.com/calimero-network/swift-sdk/edit/master/docs/',
      },
      head: [
        { tag: 'meta', attrs: { name: 'theme-color', content: '#09090b' } },
      ],
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/calimero-network/swift-sdk',
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
          label: 'Guides',
          items: [
            'guides/calls-and-events',
            'guides/app-session',
            'guides/consent',
            'guides/sealed-transport',
            'guides/tee-trust',
            'guides/agent-gate',
          ],
        },
        {
          label: 'Reference',
          items: ['reference/calimero-client', 'reference/error-model'],
        },
      ],
    }),
  ],
});
