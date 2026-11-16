import type { StorybookConfig } from '@storybook/react-vite';
import { mergeConfig } from 'vite';
import path from 'path';
import { fileURLToPath } from 'url';

// Get the project root directory
// When Storybook runs, process.cwd() is the project root
// But if running from apps/storybook, we need to go up 2 levels
let rootDir = process.cwd();
if (rootDir.endsWith('/apps/storybook') || rootDir.endsWith('\\apps\\storybook')) {
  rootDir = path.resolve(rootDir, '../..');
}

// Fallback: try to get dirname from import.meta.url if available
if (typeof import.meta.url !== 'undefined') {
  try {
    const configDir = path.dirname(fileURLToPath(import.meta.url));
    // Config is in .storybook, so go up 3 levels to get to project root
    rootDir = path.resolve(configDir, '../../..');
  } catch {
    // Use process.cwd() fallback
  }
}

const config: StorybookConfig = {
  stories: ['../src/**/*.stories.@(js|jsx|mjs|ts|tsx)'],
  addons: [
    '@storybook/addon-onboarding',
    '@storybook/addon-docs',
    '@storybook/addon-a11y',
  ],
  framework: {
    name: '@storybook/react-vite',
    options: {},
  },
  async viteFinal(config) {
    return mergeConfig(config, {
      resolve: {
        alias: {
          '@calimero-network/mero-ui': path.resolve(rootDir, 'packages/ui/src'),
          '@calimero-network/mero-tokens': path.resolve(rootDir, 'packages/tokens/src'),
          '@calimero-network/mero-charts': path.resolve(rootDir, 'packages/charts/src'),
          '@calimero-network/mero-icons': path.resolve(rootDir, 'packages/icons/src'),
        },
      },
    });
  },
};

export default config;
