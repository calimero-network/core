import type { Meta, StoryObj } from "@storybook/react-vite";
import { Tag } from "@calimero-network/mero-ui";

const meta: Meta<typeof Tag> = {
  title: "Forms/Tag",
  component: Tag,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    children: {
      control: { type: "text" },
      description: "Tag text content",
    },
    variant: {
      control: { type: "select" },
      options: ["default", "success", "warning", "error", "info", "neutral"],
      description: "Color variant",
    },
    size: {
      control: { type: "select" },
      options: ["sm", "md", "lg"],
      description: "Size variant",
    },
    disabled: {
      control: { type: "boolean" },
      description: "Whether the tag is disabled",
    },
  },
};

export default meta;
type Story = StoryObj<typeof Tag>;

export const Default: Story = {
  args: {
    children: "Default tag",
    variant: "default",
    size: "md",
  },
};

export const WithRemove: Story = {
  args: {
    children: "Removable tag",
    variant: "default",
    size: "md",
    onRemove: () => console.log("Remove tag"),
  },
};

export const Success: Story = {
  args: {
    children: "Success",
    variant: "success",
    size: "md",
  },
};

export const Warning: Story = {
  args: {
    children: "Warning",
    variant: "warning",
    size: "md",
  },
};

export const Error: Story = {
  args: {
    children: "Error",
    variant: "error",
    size: "md",
  },
};

export const Info: Story = {
  args: {
    children: "Info",
    variant: "info",
    size: "md",
  },
};

export const Neutral: Story = {
  args: {
    children: "Neutral",
    variant: "neutral",
    size: "md",
  },
};

export const AllSizes: Story = {
  render: () => (
    <div style={{ display: "flex", alignItems: "center", gap: "12px" }}>
      <Tag size="sm">Small</Tag>
      <Tag size="md">Medium</Tag>
      <Tag size="lg">Large</Tag>
    </div>
  ),
};

export const AllVariants: Story = {
  render: () => (
    <div style={{ display: "flex", flexWrap: "wrap", gap: "8px" }}>
      <Tag variant="default">Default</Tag>
      <Tag variant="success">Success</Tag>
      <Tag variant="warning">Warning</Tag>
      <Tag variant="error">Error</Tag>
      <Tag variant="info">Info</Tag>
      <Tag variant="neutral">Neutral</Tag>
    </div>
  ),
};

export const WithRemoveAll: Story = {
  render: () => (
    <div style={{ display: "flex", flexWrap: "wrap", gap: "8px" }}>
      <Tag variant="default" onRemove={() => console.log("Remove default")}>
        Default
      </Tag>
      <Tag variant="success" onRemove={() => console.log("Remove success")}>
        Success
      </Tag>
      <Tag variant="warning" onRemove={() => console.log("Remove warning")}>
        Warning
      </Tag>
      <Tag variant="error" onRemove={() => console.log("Remove error")}>
        Error
      </Tag>
      <Tag variant="info" onRemove={() => console.log("Remove info")}>
        Info
      </Tag>
      <Tag variant="neutral" onRemove={() => console.log("Remove neutral")}>
        Neutral
      </Tag>
    </div>
  ),
};

export const Disabled: Story = {
  render: () => (
    <div style={{ display: "flex", flexWrap: "wrap", gap: "8px" }}>
      <Tag variant="default" disabled>
        Disabled Default
      </Tag>
      <Tag variant="success" disabled>
        Disabled Success
      </Tag>
      <Tag variant="warning" disabled>
        Disabled Warning
      </Tag>
      <Tag variant="error" disabled>
        Disabled Error
      </Tag>
    </div>
  ),
};
