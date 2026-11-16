import type { StorybookConfig } from '@storybook/react-vite';
import { mergeConfig } from 'vite';
import path from 'path';
import { fileURLToPath } from 'url';

// This storybook lives in sdks/components/mero-ui/examples/storybook and shows
// the sources of the four design-system packages next to mero-ui.
const configDir = path.dirname(fileURLToPath(import.meta.url));
const componentsDir = path.resolve(configDir, '../../../..');

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
          '@calimero-network/mero-ui': path.resolve(componentsDir, 'mero-ui/src'),
          '@calimero-network/mero-tokens': path.resolve(componentsDir, 'mero-tokens/src'),
          '@calimero-network/mero-charts': path.resolve(componentsDir, 'mero-charts/src'),
          '@calimero-network/mero-icons': path.resolve(componentsDir, 'mero-icons/src'),
        },
      },
    });
  },
};

export default config;
