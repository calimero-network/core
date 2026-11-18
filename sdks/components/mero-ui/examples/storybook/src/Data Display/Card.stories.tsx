import type { Meta, StoryObj } from "@storybook/react-vite";
import React from "react";
import {
  Card,
  CardHeader,
  CardTitle,
  CardContent,
} from "@calimero-network/mero-ui";
import { Settings } from "@calimero-network/mero-icons";

type CardProps = React.ComponentProps<typeof Card>;

const meta: Meta<typeof Card> = {
  title: "Data Display/Card",
  component: Card,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
  argTypes: {
    className: {
      control: "text",
      description: "Optional CSS class name",
    },
    tooltip: {
      control: "text",
      description: "Tooltip text to show on the info icon",
    },
    color: {
      control: "color",
      description: "Border color - defaults to neutral-600",
    },
    noBorder: {
      control: "boolean",
      description: "Remove border entirely",
    },
    variant: {
      control: "select",
      options: ["rounded", "rectangle"],
      description: "Border radius variant",
    },
    title: {
      control: "text",
      description: "Card title",
    },
    description: {
      control: "text",
      description: "Description text below title",
    },
    onClick: {
      action: "clicked",
      description: "Click handler for clickable cards",
    },
    // Hide props that don't make sense as controls
    children: {
      table: {
        disable: true,
      },
    },
    tooltipIcon: {
      table: {
        disable: true,
      },
    },
    icon: {
      table: {
        disable: true,
      },
    },
    actions: {
      table: {
        disable: true,
      },
    },
    style: {
      table: {
        disable: true,
      },
    },
  },
};

export default meta;

type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    title: "Card Title",
    description: "This is a card description that can be customized via controls.",
    tooltip: "This is a helpful tooltip",
    color: "#404040",
    noBorder: false,
    variant: "rounded",
  },
  render: (args: CardProps) => (
    <div style={{ maxWidth: "400px" }}>
      <Card {...args} icon={<Settings size={20} />} />
    </div>
  ),
};

export const Basic: Story = {
  render: () => (
    <div style={{ maxWidth: "400px" }}>
      <Card>
        <CardHeader>
          <CardTitle>Basic Card</CardTitle>
        </CardHeader>
        <CardContent>
          <p style={{ color: "white", margin: 0 }}>
            This is a basic card with a title and content.
          </p>
        </CardContent>
      </Card>
    </div>
  ),
};

export const WithTitleIconDescription: Story = {
  args: {
    title: "Security",
    description: "Enterprise-grade security features and hardening.",
    color: "#404040",
    noBorder: false,
    variant: "rounded",
  },
  render: (args: CardProps) => (
    <div style={{ maxWidth: "420px" }}>
      <Card {...args} icon={<Settings size={20} />} />
    </div>
  ),
};

export const Variants: Story = {
  render: () => (
    <div style={{ display: "flex", gap: "24px", maxWidth: "800px" }}>
      <div style={{ flex: 1 }}>
        <h3 style={{ color: "white", marginBottom: "12px" }}>
          Rounded (Default)
        </h3>
        <Card variant="rounded" color="#3B82F6">
          <CardHeader>
            <CardTitle>Rounded Card</CardTitle>
          </CardHeader>
          <CardContent>
            <p style={{ color: "white", margin: 0 }}>
              This card uses the default rounded variant with 16px border
              radius.
            </p>
          </CardContent>
        </Card>
      </div>

      <div style={{ flex: 1 }}>
        <h3 style={{ color: "white", marginBottom: "12px" }}>Rectangle</h3>
        <Card variant="rectangle" color="#10B981">
          <CardHeader>
            <CardTitle>Rectangle Card</CardTitle>
          </CardHeader>
          <CardContent>
            <p style={{ color: "white", margin: 0 }}>
              This card uses the rectangle variant with 4px border radius.
            </p>
          </CardContent>
        </Card>
      </div>
    </div>
  ),
};

export const WithActions: Story = {
  args: {
    title: "Deploy",
    description: "Prepare and deploy your application.",
    color: "#404040",
    noBorder: false,
    variant: "rounded",
  },
  render: (args: CardProps) => (
    <div style={{ maxWidth: "420px" }}>
      <Card
        {...args}
        icon={<Settings size={20} />}
        actions={
          <button
            style={{
              background: "#A5FF11",
              color: "#000",
              border: 0,
              borderRadius: 8,
              padding: "6px 10px",
              fontWeight: 600,
            }}
          >
            Action
          </button>
        }
      />
    </div>
  ),
};

export const Clickable: Story = {
  args: {
    title: "Explore Apps",
    description: "Browse featured applications in the registry.",
    color: "#404040",
    noBorder: false,
    variant: "rounded",
  },
  render: (args: CardProps) => (
    <div style={{ maxWidth: "420px" }}>
      <Card
        {...args}
        icon={<Settings size={20} />}
        onClick={() => console.log("card clicked")}
      />
    </div>
  ),
};

export const VariantComparison: Story = {
  render: () => (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fit, minmax(200px, 1fr))",
        gap: "16px",
        maxWidth: "600px",
      }}
    >
      <Card variant="rounded" color="#F59E0B">
        <CardContent>
          <div style={{ color: "white", textAlign: "center", padding: "8px" }}>
            <strong>Rounded</strong>
            <br />
            <small>16px radius</small>
          </div>
        </CardContent>
      </Card>

      <Card variant="rectangle" color="#EF4444">
        <CardContent>
          <div style={{ color: "white", textAlign: "center", padding: "8px" }}>
            <strong>Rectangle</strong>
            <br />
            <small>4px radius</small>
          </div>
        </CardContent>
      </Card>

      <Card variant="rounded" color="#8B5CF6" noBorder>
        <CardContent>
          <div style={{ color: "white", textAlign: "center", padding: "8px" }}>
            <strong>Rounded</strong>
            <br />
            <small>No border</small>
          </div>
        </CardContent>
      </Card>

      <Card variant="rectangle" color="#06B6D4" noBorder>
        <CardContent>
          <div style={{ color: "white", textAlign: "center", padding: "8px" }}>
            <strong>Rectangle</strong>
            <br />
            <small>No border</small>
          </div>
        </CardContent>
      </Card>
    </div>
  ),
};
