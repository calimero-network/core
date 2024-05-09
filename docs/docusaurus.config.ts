import type { Config } from "@docusaurus/types";
import type * as Preset from "@docusaurus/preset-classic";

const config: Config = {
  title: "Calimero 2.0",
  tagline:
    "Calimero Network is a framework which enables building fully decentralized applications, ensuring everyone's data privacy.",
  favicon: "img/favicon.ico",
  url: "https://calimero-network.github.io",
  baseUrl: "/core/",
  organizationName: "calimero-network",
  projectName: "core",
  onBrokenLinks: "throw",
  onBrokenMarkdownLinks: "warn",
  i18n: {
    defaultLocale: "en",
    locales: ["en"],
  },
  headTags: [
    {
      tagName: "script",
      attributes: {
        "data-collect-dnt": "true",
        src: "https://scripts.simpleanalyticscdn.com/latest.js",
        async: "async",
        defer: "defer",
      },
    },
  ],
  presets: [
    [
      "classic",
      {
        docs: {
          routeBasePath: "/", // Serve the docs at the site's root
          breadcrumbs: true,
        },
        blog: false,
        theme: {
          customCss: "./src/css/custom.css",
        },
      } satisfies Preset.Options,
    ],
  ],
  themeConfig: {
    colorMode: {
      disableSwitch: true,
    },
    docs: {
      sidebar: {
        hideable: true,
      },
    },
    navbar: {
      style: "dark",
      logo: {
        alt: "Calimero Network",
        src: "img/logo.svg",
      },
      items: [
        {
          href: "https://github.com/calimero-network/core",
          label: "GitHub",
          position: "right",
        },
      ],
    },
    footer: {
      style: "dark",
      copyright: `Copyright © ${new Date().getFullYear()} Calimero Limited LLC.`,
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
