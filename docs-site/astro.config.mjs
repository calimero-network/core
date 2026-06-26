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
      sidebar: [
        {
          label: 'Start here',
          items: [{ label: 'Overview', slug: 'index' }],
        },
        {
          label: 'Build',
          autogenerate: { directory: 'build' },
        },
        {
          label: 'Operate',
          autogenerate: { directory: 'operate' },
        },
        {
          label: 'Protocol Reference',
          autogenerate: { directory: 'protocol' },
        },
        {
          label: 'Contribute',
          autogenerate: { directory: 'contribute' },
        },
      ],
    }),
  ],
});
