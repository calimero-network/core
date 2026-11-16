import type { Meta, StoryObj } from "@storybook/react-vite";
import { Badge, StatusBadge } from "@calimero-network/mero-ui";

const meta: Meta<typeof Badge> = {
  title: "Data Display/Badge",
  component: Badge,
  parameters: { layout: "centered" },
  argTypes: {
    variant: {
      control: { type: "select" },
      options: [
        "default",
        "success",
        "warning",
        "error",
        "info",
        "outline",
        "neutral",
      ],
    },
    size: {
      control: { type: "select" },
      options: ["sm", "md", "lg"],
    },
  },
};

export default meta;

type Story = StoryObj<typeof Badge>;

export const Default: Story = { args: { children: "Default" } };
export const Success: Story = {
  args: { children: "Success", variant: "success" },
};
export const Warning: Story = {
  args: { children: "Warning", variant: "warning" },
};
export const Error: Story = { args: { children: "Error", variant: "error" } };
export const Info: Story = { args: { children: "Info", variant: "info" } };
export const Outline: Story = {
  args: { children: "Outline", variant: "outline" },
};
export const Neutral: Story = {
  args: { children: "Neutral", variant: "neutral" },
};
export const WithDot: Story = {
  args: { children: "Syncing", variant: "info", dot: true },
};

export const WithIcon: Story = {
  args: {
    children: "With Icon",
    variant: "success",
    icon: "check-circle",
  },
};

export const AllSizes: Story = {
  render: () => (
    <div style={{ display: "flex", alignItems: "center", gap: "12px" }}>
      <Badge size="sm">Small</Badge>
      <Badge size="md">Medium</Badge>
      <Badge size="lg">Large</Badge>
    </div>
  ),
};

export const AllVariants: Story = {
  render: () => (
    <div style={{ display: "flex", flexWrap: "wrap", gap: "8px" }}>
      <Badge variant="default">Default</Badge>
      <Badge variant="success">Success</Badge>
      <Badge variant="warning">Warning</Badge>
      <Badge variant="error">Error</Badge>
      <Badge variant="info">Info</Badge>
      <Badge variant="outline">Outline</Badge>
      <Badge variant="neutral">Neutral</Badge>
    </div>
  ),
};

// StatusBadge stories
const statusMeta: Meta<typeof StatusBadge> = {
  title: "Data Display/Status Badge",
  component: StatusBadge,
  parameters: { layout: "centered" },
  argTypes: {
    status: {
      control: { type: "select" },
      options: [
        "active",
        "inactive",
        "pending",
        "error",
        "success",
        "warning",
        "info",
      ],
    },
    size: {
      control: { type: "select" },
      options: ["sm", "md", "lg"],
    },
  },
};

export const StatusActive: Story = {
  args: { status: "active" },
  component: StatusBadge,
};
export const StatusInactive: Story = {
  args: { status: "inactive" },
  component: StatusBadge,
};
export const StatusPending: Story = {
  args: { status: "pending" },
  component: StatusBadge,
};
export const StatusError: Story = {
  args: { status: "error" },
  component: StatusBadge,
};
export const StatusSuccess: Story = {
  args: { status: "success" },
  component: StatusBadge,
};
export const StatusWarning: Story = {
  args: { status: "warning" },
  component: StatusBadge,
};
export const StatusInfo: Story = {
  args: { status: "info" },
  component: StatusBadge,
};

export const AllStatuses: Story = {
  render: () => (
    <div style={{ display: "flex", flexWrap: "wrap", gap: "8px" }}>
      <StatusBadge status="active" />
      <StatusBadge status="inactive" />
      <StatusBadge status="pending" />
      <StatusBadge status="error" />
      <StatusBadge status="success" />
      <StatusBadge status="warning" />
      <StatusBadge status="info" />
    </div>
  ),
  component: StatusBadge,
};
