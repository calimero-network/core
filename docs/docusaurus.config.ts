import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';

const config: Config = {
  title: 'My Site',
  tagline: 'Dinosaurs are cool',
  favicon: 'img/favicon.ico',

  url: 'https://calimero-network.github.io',
  baseUrl: '/core/',
  organizationName: 'calimero-network',
  projectName: 'core',

  onBrokenLinks: 'throw',
  onBrokenMarkdownLinks: 'warn',

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      {
        docs: {
          routeBasePath: '/', // Serve the docs at the site's root
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    colorMode: {
      disableSwitch: true
    },
    navbar: {
      style: 'dark',
      logo: {
        alt: 'Calimero Network',
        src: 'img/logo.svg',
      },
      items: [
        {
          href: 'https://github.com/calimero-network/core',
          label: 'GitHub',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Community',
          items: [
            {
              label: 'Stack Overflow (TBD)',
              href: 'https://stackoverflow.com/questions/tagged/calimero-network',
            },
            {
              label: 'Discord',
              href: 'https://discord.gg/bp7uKv9kBv',
            },
            {
              label: 'Twitter',
              href: 'https://twitter.com/CalimeroNetwork',
            },
            {
              label: 'Telegram',
              href: 'https://t.me/+_6h-gJlnXO83OGVk',
            }
          ],
        },
        {
          title: 'More',
          items: [
            {
              label: 'Blog',
              to: 'https://www.calimero.network/blogs',
            },
            {
              label: 'GitHub',
              href: 'https://github.com/calimero-network/core',
            },
          ],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} Calimero Limited LLC.`,
    }
  } satisfies Preset.ThemeConfig,
};

export default config;
