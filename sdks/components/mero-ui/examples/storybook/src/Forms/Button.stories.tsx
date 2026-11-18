import type { Meta, StoryObj } from "@storybook/react-vite";
import { Button, Spinner } from "@calimero-network/mero-ui";
import { cssVariables } from "@calimero-network/mero-tokens";

const withTokens = (Story: any) => (
  <>
    <style>{cssVariables}</style>
    <Story />
  </>
);

const meta: Meta<typeof Button> = {
  title: "Forms/Button",
  component: Button,
  decorators: [withTokens],
  parameters: {
    layout: "centered",
    docs: {
      description: {
        component:
          "A versatile, accessible button component with multiple variants, sizes, and states. Supports icons, full-width layout, and border radius control. For loading states, compose with Spinner + disabled + aria-busy.",
      },
    },
  },
  tags: ["autodocs"],
  argTypes: {
    variant: {
      control: "select",
      options: ["primary", "secondary", "success", "warning", "error", "info", "outline", "ghost", "destructive"],
      description: "Visual style variant of the button",
    },
    size: {
      control: "select",
      options: ["sm", "md", "lg", "xl"],
      description: "Size of the button",
    },
    disabled: {
      control: "boolean",
      description: "Whether the button is disabled",
    },
    fullWidth: {
      control: "boolean",
      description: "Make button span full width of container",
    },
    rounded: {
      control: "boolean",
      description: "Whether button has rounded corners (default: true, 12px radius)",
    },
    leftIcon: {
      description: "Icon to display on the left side of the button text",
    },
    rightIcon: {
      description: "Icon to display on the right side of the button text",
    },
  },
};

export default meta;

type Story = StoryObj<typeof Button>;

/**
 * Primary button states: default and disabled.
 * The primary variant uses the brand color with black text.
 */
export const Primary: Story = {
  render: () => (
    <div style={{ display: "flex", gap: 16 }}>
      <Button>Click me</Button>
      <Button disabled>Disabled</Button>
    </div>
  ),
};

/**
 * All available button variants with their disabled states.
 * Includes semantic variants (success, warning, error, info) and style variants (outline, ghost, destructive).
 */
export const States: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      <div style={{ display: "flex", gap: 16 }}>
        <Button>Primary</Button>
        <Button disabled>Primary Disabled</Button>
      </div>
      <div style={{ display: "flex", gap: 16 }}>
        <Button variant="secondary">Secondary</Button>
        <Button variant="secondary" disabled>
          Secondary Disabled
        </Button>
      </div>
      <div style={{ display: "flex", gap: 16 }}>
        <Button variant="success">Success</Button>
        <Button variant="warning">Warning</Button>
        <Button variant="error">Error</Button>
        <Button variant="info">Info</Button>
        <Button variant="outline">Outline</Button>
        <Button variant="ghost">Ghost</Button>
        <Button variant="destructive">Destructive</Button>
      </div>
    </div>
  ),
};

/**
 * All available button sizes: sm (32px), md (40px), lg (48px), and xl (56px).
 */
export const Sizes: Story = {
  render: () => (
    <div style={{ display: "flex", alignItems: "center", gap: 16 }}>
      <Button size="sm">Small</Button>
      <Button size="md">Medium</Button>
      <Button size="lg">Large</Button>
      <Button size="xl">XLarge</Button>
    </div>
  ),
};

const Icon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M12 2l3 7h7l-5.5 4 2 7-6.5-4.5L5.5 20l2-7L2 9h7l3-7z" />
  </svg>
);

/**
 * Buttons with icons on the left or right side.
 */
export const WithIcons: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      <div style={{ display: "flex", gap: 16 }}>
        <Button leftIcon={<Icon />}>Left icon</Button>
        <Button rightIcon={<Icon />}>Right icon</Button>
      </div>
      <div style={{ display: "flex", gap: 16 }}>
        <Button size="lg" leftIcon={<Icon />}>Large with icon</Button>
        <Button variant="secondary" rightIcon={<Icon />}>Secondary with icon</Button>
      </div>
    </div>
  ),
};

/**
 * Full-width button that spans the entire width of its container.
 * Useful for forms and modal actions.
 */
export const FullWidth: Story = {
  render: () => (
    <div style={{ width: 360 }}>
      <Button fullWidth>Full width button</Button>
    </div>
  ),
};

/**
 * Composed loading pattern using Spinner and disabled state.
 * Prefer this approach over the deprecated `loading` prop.
 * Use small, thin spinners that match the button's text color.
 */
export const ComposedLoading: Story = {
  render: () => (
    <div style={{ display: "flex", gap: 16, alignItems: "center" }}>
      <Button disabled aria-busy>
        <Spinner size="xs" color="current" thickness="thin" style={{ marginRight: 8 }} />
        Saving...
      </Button>
      <Button variant="secondary" disabled aria-busy>
        <Spinner size="xs" color="current" thickness="thin" style={{ marginRight: 8 }} />
        Loading
      </Button>
    </div>
  ),
};

/**
 * Border radius control: rounded (default, 12px) vs not rounded (0px, sharp corners).
 * The rounded prop allows you to control the corner styling of buttons.
 */
export const BorderRadius: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      <div style={{ display: "flex", gap: 16, alignItems: "center" }}>
        <Button rounded={true}>Rounded (default)</Button>
        <Button rounded={false}>Not Rounded</Button>
      </div>
      <div style={{ display: "flex", gap: 16, alignItems: "center" }}>
        <Button variant="secondary" rounded={true}>Rounded Secondary</Button>
        <Button variant="secondary" rounded={false}>Not Rounded Secondary</Button>
      </div>
    </div>
  ),
};
