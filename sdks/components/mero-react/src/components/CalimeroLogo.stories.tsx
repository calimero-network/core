import type { Meta, StoryObj } from '@storybook/react';
import { CalimeroLogo } from './CalimeroLogo';

const meta: Meta<typeof CalimeroLogo> = {
  title: 'Components/CalimeroLogo',
  component: CalimeroLogo,
  tags: ['autodocs'],
  argTypes: {
    size: {
      control: { type: 'range', min: 16, max: 256, step: 4 },
      description: 'Width and height in pixels (or any CSS length string).',
    },
    color: {
      control: 'color',
      description: 'Override fill colour. Defaults to `currentColor`.',
    },
  },
};
export default meta;

type Story = StoryObj<typeof CalimeroLogo>;

export const Default: Story = {
  args: { size: 128 },
};

export const Tiny: Story = {
  args: { size: 16, color: '#a5ff11' },
};

export const Branded: Story = {
  args: { size: 96, color: '#a5ff11' },
};

export const Pink: Story = {
  args: { size: 96, color: '#ff4081' },
};

export const Blue: Story = {
  args: { size: 96, color: '#3b82f6' },
};

export const Inherits: Story = {
  args: { size: 96 },
  decorators: [
    (Story) => (
      <div style={{ color: '#fb923c' }}>
        <Story />
      </div>
    ),
  ],
  parameters: {
    docs: {
      description: {
        story:
          'No `color` prop — inherits from `currentColor`. Wrap the logo in any element with a `color` style and the logo follows.',
      },
    },
  },
};
