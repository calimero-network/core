import type { Preview } from '@storybook/react';
import React from 'react';
import { MockMeroProvider } from '../src/test-utils/MockMeroProvider';

// Side-effect: ensure the bundled component CSS is injected into Storybook's
// iframe. Importing the entry pulls `./components/styles.css` along with it.
import '../src/index';

const preview: Preview = {
  parameters: {
    backgrounds: {
      default: 'dark',
      values: [
        { name: 'dark', value: '#0d1117' },
        { name: 'light', value: '#ffffff' },
        { name: 'mid', value: '#1f2937' },
      ],
    },
    controls: {
      matchers: {
        color: /(background|color|primary|border|overlay|text|error)$/i,
      },
      expanded: true,
    },
    layout: 'centered',
  },
  decorators: [
    (Story, context) => {
      // Stories may opt into the mock context via parameters.mockMero
      const cfg = context.parameters.mockMero ?? {};
      return (
        <MockMeroProvider
          isAuthenticated={cfg.isAuthenticated ?? false}
          nodeUrl={cfg.nodeUrl ?? 'https://node.example.com'}
          servedByNode={cfg.servedByNode ?? false}
        >
          <Story />
        </MockMeroProvider>
      );
    },
  ],
};

export default preview;
