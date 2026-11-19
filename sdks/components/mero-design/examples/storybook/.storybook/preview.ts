import type { Preview } from "@storybook/react-vite";
import { cssVariables } from "@calimero-network/mero-tokens";

// Inject CSS variables directly into the document
const style = document.createElement("style");
style.textContent =
  cssVariables +
  `
  /* Calimero brand theme - dark background */
  body {
    font-family: 'Power Grotesk', 'Inter', system-ui, sans-serif;
    background: var(--color-background-brand);
    color: #FFFFFF;
    min-height: 100vh;
  }
  
  /* Dark theme for charts */
  .dark {
    background: var(--color-background-brand);
    color: #FFFFFF;
  }
  
  /* Dark theme for Storybook UI (sidebar) */
  .os-content {
    background: var(--color-background-brand) !important;
  }
  
  /* Dark theme for Storybook docs */
  .docs-story {
    background: var(--color-background-brand) !important;
  }
  
  .docs-story > div {
    background: var(--color-background-brand) !important;
  }
  
  /* Dark theme for docs content */
  .sb-docs-content {
    background: var(--color-background-brand) !important;
    color: #FFFFFF !important;
  }
  
  .sb-docs-content h1,
  .sb-docs-content h2,
  .sb-docs-content h3,
  .sb-docs-content h4,
  .sb-docs-content h5,
  .sb-docs-content h6 {
    color: #FFFFFF !important;
  }
  
  .sb-docs-content p,
  .sb-docs-content li,
  .sb-docs-content td,
  .sb-docs-content th {
    color: #FFFFFF !important;
  }
  
  /* Dark theme for docs tables */
  .sb-docs-content table {
    background: var(--color-background-primary) !important;
    border: 1px solid var(--color-neutral-600) !important;
  }
  
  .sb-docs-content th {
    background: var(--color-background-secondary) !important;
    border-bottom: 1px solid var(--color-neutral-600) !important;
  }
  
  .sb-docs-content td {
    border-bottom: 1px solid var(--color-neutral-700) !important;
  }
  
  /* Dark theme for code blocks */
  .sb-docs-content pre {
    background: var(--color-background-primary) !important;
    border: 1px solid var(--color-neutral-600) !important;
  }
  
  .sb-docs-content code {
    background: var(--color-background-primary) !important;
    color: #FFFFFF !important;
  }
  
`;
document.head.appendChild(style);

const preview: Preview = {
  parameters: {
    actions: { argTypesRegex: "^on[A-Z].*" },
    controls: {
      matchers: {
        color: /(background|color)$/i,
        date: /Date$/,
      },
    },
    backgrounds: {
      options: {
        // 👇 Calimero brand theme options - exact brand colors (hex values, not CSS vars)
        brand: {
          name: "Brand",
          value: "#1F2A15",
        },
        primary: { name: "Primary", value: "#1A1A1A" },
        secondary: {
          name: "Secondary",
          value: "#2A2A2A",
        },
        tertiary: {
          name: "Tertiary",
          value: "#3A3A3A",
        },
        accent: { name: "Brand Accent", value: "#A5FF11" },
        neutral: { name: "Neutral 900", value: "#131215" },
        light: { name: "Light", value: "#FFFFFF" },
      },
    },
  },
  initialGlobals: {
    backgrounds: { value: "brand" },
  },
};

export default preview;
