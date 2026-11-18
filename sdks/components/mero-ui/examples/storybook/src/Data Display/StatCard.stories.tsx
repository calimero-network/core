import type { Meta, StoryObj } from "@storybook/react-vite";
import { StatCard } from "@calimero-network/mero-ui";
import { Icon } from "@calimero-network/mero-ui";

const meta: Meta<typeof StatCard> = {
  title: "Data Display/Stat Card",
  component: StatCard,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    title: {
      control: { type: "text" },
      description: "Card title",
    },
    value: {
      control: { type: "text" },
      description: "Main value to display",
    },
    subtitle: {
      control: { type: "text" },
      description: "Subtitle text",
    },
    trend: {
      control: { type: "object" },
      description: "Trend information",
    },
    color: {
      control: { type: "select" },
      options: ["primary", "success", "warning", "error", "info", "neutral"],
      description: "Color variant",
    },
    size: {
      control: { type: "select" },
      options: ["small", "medium", "large"],
      description: "Card size",
    },
    loading: {
      control: { type: "boolean" },
      description: "Show loading state",
    },
    onClick: {
      action: "clicked",
      description: "Click handler",
    },
  },
};

export default meta;
type Story = StoryObj<typeof StatCard>;

export const Default: Story = {
  args: {
    title: "Total Users",
    value: "1,234",
    subtitle: "Active users this month",
  },
};

export const WithTrend: Story = {
  args: {
    title: "Revenue",
    value: "$45,678",
    subtitle: "This month",
    trend: {
      value: 12.5,
      direction: "up",
      period: "vs last month",
    },
  },
};

export const WithIcon: Story = {
  args: {
    title: "Downloads",
    value: "2,456",
    subtitle: "This week",
    icon: <Icon name="download" size={24} />,
    trend: {
      value: 8.2,
      direction: "up",
      period: "vs last week",
    },
  },
};

export const Success: Story = {
  args: {
    title: "Conversion Rate",
    value: "24.5%",
    subtitle: "This quarter",
    color: "success",
    trend: {
      value: 3.2,
      direction: "up",
      period: "vs last quarter",
    },
  },
};

export const Warning: Story = {
  args: {
    title: "Server Load",
    value: "78%",
    subtitle: "Current usage",
    color: "warning",
    trend: {
      value: 5.1,
      direction: "up",
      period: "vs last hour",
    },
  },
};

export const Error: Story = {
  args: {
    title: "Error Rate",
    value: "2.3%",
    subtitle: "Last 24 hours",
    color: "error",
    trend: {
      value: 1.8,
      direction: "down",
      period: "vs yesterday",
    },
  },
};

export const Info: Story = {
  args: {
    title: "API Calls",
    value: "156K",
    subtitle: "This month",
    color: "info",
    trend: {
      value: 15.7,
      direction: "up",
      period: "vs last month",
    },
  },
};

export const Neutral: Story = {
  args: {
    title: "Page Views",
    value: "89,234",
    subtitle: "All time",
    color: "neutral",
    trend: {
      value: 0,
      direction: "neutral",
      period: "No change",
    },
  },
};

export const Small: Story = {
  args: {
    title: "Orders",
    value: "456",
    subtitle: "Today",
    size: "small",
    trend: {
      value: 12.3,
      direction: "up",
      period: "vs yesterday",
    },
  },
};

export const Large: Story = {
  args: {
    title: "Total Sales",
    value: "$1,234,567",
    subtitle: "This year",
    size: "large",
    icon: <Icon name="trending-up" size={32} />,
    trend: {
      value: 25.8,
      direction: "up",
      period: "vs last year",
    },
  },
};

export const Loading: Story = {
  args: {
    title: "Loading Data",
    value: "0",
    subtitle: "Please wait...",
    loading: true,
  },
};

export const Clickable: Story = {
  args: {
    title: "View Details",
    value: "Click me",
    subtitle: "Interactive card",
    icon: <Icon name="arrow-right" size={20} />,
    onClick: () => alert("Card clicked!"),
  },
};

export const Grid: Story = {
  render: () => (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fit, minmax(250px, 1fr))",
        gap: "16px",
      }}
    >
      <StatCard
        title="Total Users"
        value="1,234"
        subtitle="Active users"
        icon={<Icon name="users" size={20} />}
        trend={{ value: 12.5, direction: "up", period: "vs last month" }}
      />
      <StatCard
        title="Revenue"
        value="$45,678"
        subtitle="This month"
        color="success"
        icon={<Icon name="dollar-sign" size={20} />}
        trend={{ value: 8.2, direction: "up", period: "vs last month" }}
      />
      <StatCard
        title="Orders"
        value="456"
        subtitle="Today"
        color="info"
        icon={<Icon name="shopping-cart" size={20} />}
        trend={{ value: 3.1, direction: "down", period: "vs yesterday" }}
      />
      <StatCard
        title="Error Rate"
        value="0.2%"
        subtitle="Last 24h"
        color="error"
        icon={<Icon name="alert-circle" size={20} />}
        trend={{ value: 0.1, direction: "down", period: "vs yesterday" }}
      />
    </div>
  ),
};
