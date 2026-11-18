import type { Meta, StoryObj } from "@storybook/react-vite";
import { MetricCard } from "@calimero-network/mero-ui";
import { Icon } from "@calimero-network/mero-ui";

const meta: Meta<typeof MetricCard> = {
  title: "Data Display/Metric Card",
  component: MetricCard,
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
    sparkline: {
      control: { type: "object" },
      description: "Sparkline data array",
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
type Story = StoryObj<typeof MetricCard>;

// Sample sparkline data
const sampleSparkline = [
  { value: 100, timestamp: "2024-01-01" },
  { value: 120, timestamp: "2024-01-02" },
  { value: 110, timestamp: "2024-01-03" },
  { value: 140, timestamp: "2024-01-04" },
  { value: 130, timestamp: "2024-01-05" },
  { value: 160, timestamp: "2024-01-06" },
  { value: 150, timestamp: "2024-01-07" },
  { value: 180, timestamp: "2024-01-08" },
  { value: 170, timestamp: "2024-01-09" },
  { value: 200, timestamp: "2024-01-10" },
];

export const Default: Story = {
  args: {
    title: "Total Revenue",
    value: "$45,678",
    subtitle: "This month",
    sparkline: sampleSparkline,
  },
};

export const WithTrend: Story = {
  args: {
    title: "Active Users",
    value: "2,456",
    subtitle: "Currently online",
    sparkline: sampleSparkline,
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
    value: "15,234",
    subtitle: "This week",
    icon: <Icon name="download" size={24} />,
    sparkline: sampleSparkline,
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
    sparkline: sampleSparkline,
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
    sparkline: sampleSparkline,
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
    sparkline: sampleSparkline,
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
    sparkline: sampleSparkline,
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
    sparkline: sampleSparkline,
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
    sparkline: sampleSparkline,
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
    sparkline: sampleSparkline,
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
    sparkline: sampleSparkline,
    onClick: () => alert("Card clicked!"),
  },
};

export const WithThreshold: Story = {
  args: {
    title: "CPU Usage",
    value: "85%",
    subtitle: "Current usage",
    color: "warning",
    sparkline: sampleSparkline,
    threshold: {
      value: 80,
      type: "above",
      color: "var(--color-semantic-error)",
    },
  },
};

export const WithCustomSparklineColor: Story = {
  args: {
    title: "Custom Sparkline",
    value: "1,234",
    subtitle: "With custom color",
    sparkline: sampleSparkline,
    sparklineColor: "var(--color-semantic-success)",
    trend: {
      value: 8.5,
      direction: "up",
      period: "vs last week",
    },
  },
};

export const WithoutSparkline: Story = {
  args: {
    title: "Simple Metric",
    value: "42",
    subtitle: "No sparkline",
    showSparkline: false,
    trend: {
      value: 5.2,
      direction: "up",
      period: "vs last month",
    },
  },
};

export const Dashboard: Story = {
  render: () => (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fit, minmax(250px, 1fr))",
        gap: "16px",
      }}
    >
      <MetricCard
        title="Total Revenue"
        value="$45,678"
        subtitle="This month"
        icon={<Icon name="dollar-sign" size={20} />}
        sparkline={sampleSparkline}
        trend={{ value: 12.5, direction: "up", period: "vs last month" }}
      />
      <MetricCard
        title="Active Users"
        value="2,456"
        subtitle="Currently online"
        color="success"
        icon={<Icon name="users" size={20} />}
        sparkline={sampleSparkline}
        trend={{ value: 8.2, direction: "up", period: "vs last week" }}
      />
      <MetricCard
        title="Orders"
        value="1,234"
        subtitle="Today"
        color="info"
        icon={<Icon name="shopping-cart" size={20} />}
        sparkline={sampleSparkline}
        trend={{ value: 3.1, direction: "down", period: "vs yesterday" }}
      />
      <MetricCard
        title="Error Rate"
        value="0.2%"
        subtitle="Last 24h"
        color="error"
        icon={<Icon name="alert-circle" size={20} />}
        sparkline={sampleSparkline}
        trend={{ value: 0.1, direction: "down", period: "vs yesterday" }}
      />
    </div>
  ),
};

export const PerformanceMetrics: Story = {
  render: () => (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))",
        gap: "20px",
      }}
    >
      <MetricCard
        title="Response Time"
        value="245ms"
        subtitle="Average response time"
        color="success"
        icon={<Icon name="clock" size={24} />}
        sparkline={sampleSparkline}
        trend={{ value: 15.2, direction: "down", period: "vs last week" }}
        threshold={{
          value: 500,
          type: "above",
          color: "var(--color-semantic-warning)",
        }}
      />
      <MetricCard
        title="Throughput"
        value="1,234 req/s"
        subtitle="Requests per second"
        color="info"
        icon={<Icon name="activity" size={24} />}
        sparkline={sampleSparkline}
        trend={{ value: 22.1, direction: "up", period: "vs last week" }}
      />
      <MetricCard
        title="Uptime"
        value="99.9%"
        subtitle="Service availability"
        color="success"
        icon={<Icon name="check-circle" size={24} />}
        sparkline={sampleSparkline}
        trend={{ value: 0.1, direction: "up", period: "vs last month" }}
      />
    </div>
  ),
};
