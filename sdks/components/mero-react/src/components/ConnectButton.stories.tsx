import type { Meta, StoryObj } from '@storybook/react';
import { ConnectButton } from './ConnectButton';
import type { MeroTheme } from '../theme';

/**
 * ConnectButton stories use the `MockMeroProvider` decorator wired in
 * `.storybook/preview.tsx`. Set `parameters.mockMero.isAuthenticated` on a
 * story to flip between signed out and signed in.
 *
 * Theme tokens are exposed as flat top-level args (e.g. `primary`,
 * `radius`) so each one shows up as its own colour-picker / text input in
 * the Storybook controls panel. The render function rebuilds a partial
 * `MeroTheme` from whatever's set.
 */

interface FlatArgs {
  primary?: string;
  primaryHover?: string;
  primaryText?: string;
  background?: string;
  backgroundSecondary?: string;
  border?: string;
  text?: string;
  textSecondary?: string;
  error?: string;
  overlay?: string;
  radius?: string;
  logoOnly?: boolean;
  label?: string;
}

const colorArg = { control: 'color' as const };

const meta: Meta<FlatArgs> = {
  title: 'Components/ConnectButton',
  component: ConnectButton as never,
  tags: ['autodocs'],
  argTypes: {
    primary: { ...colorArg, description: 'Theme: primary / accent CTA colour' },
    primaryHover: { ...colorArg, description: 'Theme: primary on hover' },
    primaryText: { ...colorArg, description: 'Theme: text on top of primary' },
    background: { ...colorArg, description: 'Theme: surface background' },
    backgroundSecondary: { ...colorArg, description: 'Theme: secondary surface' },
    border: { ...colorArg, description: 'Theme: border colour' },
    text: { ...colorArg, description: 'Theme: primary text colour' },
    textSecondary: { ...colorArg, description: 'Theme: muted text' },
    error: { ...colorArg, description: 'Theme: error / danger' },
    overlay: { ...colorArg, description: 'Theme: modal backdrop' },
    radius: {
      control: 'text',
      description: 'Theme: border radius (any CSS length, e.g. 8px or 999px)',
    },
    logoOnly: {
      control: 'boolean',
      description: 'Render only the Calimero logo (square 40×40 button)',
    },
    label: {
      control: 'text',
      description: 'Override the signed-out label (default "Sign in")',
    },
  },
  render: (args) => {
    const theme: MeroTheme = {};
    (
      [
        'primary',
        'primaryHover',
        'primaryText',
        'background',
        'backgroundSecondary',
        'border',
        'text',
        'textSecondary',
        'error',
        'overlay',
        'radius',
      ] as const
    ).forEach((k) => {
      if (args[k]) (theme as Record<string, string>)[k] = args[k] as string;
    });
    return (
      <ConnectButton
        theme={Object.keys(theme).length ? theme : undefined}
        logoOnly={args.logoOnly}
        label={args.label || undefined}
      />
    );
  },
};
export default meta;

type Story = StoryObj<FlatArgs>;

export const Default: Story = { args: {} };

export const Pink: Story = {
  args: {
    primary: '#ff4081',
    primaryHover: '#e91e63',
    primaryText: '#ffffff',
  },
};

export const Blue: Story = {
  args: {
    primary: '#3b82f6',
    primaryHover: '#2563eb',
    primaryText: '#ffffff',
  },
};

export const Purple: Story = {
  args: {
    primary: '#8b5cf6',
    primaryHover: '#7c3aed',
    primaryText: '#ffffff',
  },
};

export const Pill: Story = {
  args: { radius: '999px' },
};

export const Square: Story = {
  args: { radius: '0' },
};

export const LogoOnly: Story = {
  args: { logoOnly: true },
};

export const CustomLabel: Story = {
  args: { label: 'Sign in with Calimero' },
};

export const Connected: Story = {
  args: {},
  parameters: {
    mockMero: { isAuthenticated: true },
    docs: {
      description: {
        story:
          'Authenticated state. Click to open the dropdown (account, Sign out, Sign out everywhere).',
      },
    },
  },
};

export const ConnectedThemed: Story = {
  args: {
    primary: '#ff4081',
    primaryHover: '#e91e63',
    primaryText: '#ffffff',
  },
  parameters: {
    mockMero: { isAuthenticated: true },
    docs: {
      description: {
        story: 'Connected state retains the theme colour — it should NOT go grey.',
      },
    },
  },
};
