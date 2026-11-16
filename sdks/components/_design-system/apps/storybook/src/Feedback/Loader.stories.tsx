import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  Loader,
  InlineLoader,
  FullScreenLoader,
  PageLoader,
} from "@calimero-network/mero-ui";

const meta: Meta<typeof Loader> = {
  title: "Feedback/Loader",
  component: Loader,
  parameters: {
    layout: "centered",
  },
  argTypes: {
    size: {
      control: { type: "select" },
      options: ["small", "medium", "large"],
      description: "Loader size",
    },
    variant: {
      control: { type: "select" },
      options: ["spinner", "dots", "pulse"],
      description: "Loader animation variant",
    },
    color: {
      control: { type: "color" },
      description: "Loader color",
    },
    message: {
      control: { type: "text" },
      description: "Loading message",
    },
  },
};

export default meta;
type Story = StoryObj<typeof Loader>;

export const Spinner: Story = {
  args: {
    variant: "spinner",
    size: "medium",
    color: "var(--color-brand-600)",
  },
};

export const Dots: Story = {
  args: {
    variant: "dots",
    size: "medium",
    color: "var(--color-brand-600)",
  },
};

export const Pulse: Story = {
  args: {
    variant: "pulse",
    size: "medium",
    color: "var(--color-brand-600)",
  },
};

export const WithMessage: Story = {
  args: {
    variant: "spinner",
    size: "large",
    color: "var(--color-brand-600)",
    message: "Loading your data...",
  },
};

export const Small: Story = {
  args: {
    variant: "spinner",
    size: "small",
    color: "var(--color-semantic-info)",
  },
};

export const Large: Story = {
  args: {
    variant: "spinner",
    size: "large",
    color: "var(--color-semantic-success)",
  },
};

export const InlineLoaderStory: Story = {
  render: (args) => (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "12px",
        padding: "20px",
      }}
    >
      <span style={{ color: "var(--color-neutral-300)" }}>Processing</span>
      <InlineLoader {...args} />
    </div>
  ),
  args: {
    variant: "dots",
    size: "small",
    color: "var(--color-brand-600)",
  },
};

export const InlineWithTextStory: Story = {
  render: (args) => (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: "12px",
        padding: "20px",
      }}
    >
      <span style={{ color: "var(--color-neutral-300)" }}>Saving changes</span>
      <InlineLoader {...args} />
    </div>
  ),
  args: {
    variant: "spinner",
    size: "small",
    color: "var(--color-semantic-success)",
  },
};

export const FullScreenLoaderStory: Story = {
  render: (args) => (
    <div
      style={{
        position: "relative",
        height: "400px",
        background: "var(--color-background-primary)",
        borderRadius: "8px",
        overflow: "hidden",
      }}
    >
      <FullScreenLoader {...args} />
    </div>
  ),
  args: {
    message: "Loading application...",
    variant: "spinner",
    color: "var(--color-brand-600)",
  },
};

export const PageLoaderStory: Story = {
  render: (args) => (
    <div
      style={{
        position: "relative",
        height: "400px",
        background: "var(--color-background-primary)",
        borderRadius: "8px",
        overflow: "hidden",
      }}
    >
      <PageLoader {...args} />
    </div>
  ),
  args: {
    message: "Loading page...",
    variant: "dots",
    color: "var(--color-brand-600)",
  },
};

export const DifferentColors: Story = {
  render: () => (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: "24px",
        alignItems: "center",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: "12px" }}>
        <span style={{ color: "var(--color-neutral-300)" }}>Brand:</span>
        <Loader
          variant="spinner"
          size="medium"
          color="var(--color-brand-600)"
        />
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: "12px" }}>
        <span style={{ color: "var(--color-neutral-300)" }}>Success:</span>
        <Loader
          variant="dots"
          size="medium"
          color="var(--color-semantic-success)"
        />
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: "12px" }}>
        <span style={{ color: "var(--color-neutral-300)" }}>Warning:</span>
        <Loader
          variant="pulse"
          size="medium"
          color="var(--color-semantic-warning)"
        />
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: "12px" }}>
        <span style={{ color: "var(--color-neutral-300)" }}>Error:</span>
        <Loader
          variant="spinner"
          size="medium"
          color="var(--color-semantic-error)"
        />
      </div>
    </div>
  ),
};
