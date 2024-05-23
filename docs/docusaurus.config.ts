import type { Config } from "@docusaurus/types";
import type * as Preset from "@docusaurus/preset-classic";
import { themes as prismThemes } from "prism-react-renderer";

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
          sidebarPath: "./sidebars.ts",
          routeBasePath: "/", // Serve the docs at the site's root
          breadcrumbs: true,
          showLastUpdateTime: true,
        },
        blog: false,
        theme: {
          customCss: "./src/css/custom.scss",
        },
      } satisfies Preset.Options,
    ],
  ],
  plugins: ["docusaurus-plugin-sass"],
  themeConfig: {
    colorMode: {
      disableSwitch: false,
      defaultMode: "dark",
      respectPrefersColorScheme: true,
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
          to: "/explore/intro",
          position: "left",
          label: "Explore",
        },
        {
          to: "/learn/architecture",
          position: "left",
          label: "Learn",
        },
        {
          to: "/build/quickstart",
          position: "left",
          label: "Build",
        },
        {
          to: "/contribute/github",
          position: "left",
          label: "Contribute",
        },
        {
          to: "/resources/community-and-support",
          position: "left",
          label: "Resources",
        },
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
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
      additionalLanguages: ["bash", "toml"],
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
