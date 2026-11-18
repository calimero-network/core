import type { Meta, StoryObj } from "@storybook/react-vite";
import { TrendIndicator } from "@calimero-network/mero-ui";

const meta: Meta<typeof TrendIndicator> = {
  title: "Utility/Trend Indicator",
  component: TrendIndicator,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    value: {
      control: { type: "number" },
      description: "Trend percentage value",
    },
    direction: {
      control: { type: "select" },
      options: ["up", "down", "neutral"],
      description: "Trend direction",
    },
    period: {
      control: { type: "text" },
      description: "Time period for the trend",
    },
    color: {
      control: { type: "select" },
      options: ["success", "error", "neutral", "primary"],
      description: "Color variant",
    },
    size: {
      control: { type: "select" },
      options: ["sm", "md", "lg"],
      description: "Size variant",
    },
    showIcon: {
      control: { type: "boolean" },
      description: "Whether to show the trend icon",
    },
  },
};

export default meta;
type Story = StoryObj<typeof TrendIndicator>;

export const Default: Story = {
  args: {
    value: 12.5,
    direction: "up",
    period: "vs last month",
    size: "md",
    showIcon: true,
  },
};

export const Upward: Story = {
  args: {
    value: 25.3,
    direction: "up",
    period: "vs last quarter",
    color: "success",
    size: "lg",
  },
};

export const Downward: Story = {
  args: {
    value: -8.7,
    direction: "down",
    period: "vs last week",
    color: "error",
    size: "md",
  },
};

export const Neutral: Story = {
  args: {
    value: 0,
    direction: "neutral",
    period: "no change",
    color: "neutral",
    size: "sm",
  },
};

export const WithoutIcon: Story = {
  args: {
    value: 15.2,
    direction: "up",
    period: "vs last year",
    showIcon: false,
    size: "md",
  },
};

export const AllSizes: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <TrendIndicator
        value={12.5}
        direction="up"
        period="vs last month"
        size="sm"
      />
      <TrendIndicator
        value={12.5}
        direction="up"
        period="vs last month"
        size="md"
      />
      <TrendIndicator
        value={12.5}
        direction="up"
        period="vs last month"
        size="lg"
      />
    </div>
  ),
};

export const AllColors: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <TrendIndicator
        value={12.5}
        direction="up"
        period="vs last month"
        color="success"
      />
      <TrendIndicator
        value={-8.7}
        direction="down"
        period="vs last week"
        color="error"
      />
      <TrendIndicator
        value={0}
        direction="neutral"
        period="no change"
        color="neutral"
      />
      <TrendIndicator
        value={15.2}
        direction="up"
        period="vs last year"
        color="primary"
      />
    </div>
  ),
};
