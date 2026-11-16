import type { Meta, StoryObj } from "@storybook/react-vite";
import { Avatar } from "@calimero-network/mero-ui";

const meta: Meta = {
  title: "Data Display/Avatar",
  parameters: {
    layout: "padded",
  },
};

export default meta;

export const Default: StoryObj = {
  render: () => (
    <div style={{ display: "flex", gap: "16px", alignItems: "center" }}>
      <Avatar name="John Doe" />
      <Avatar name="Jane Smith" />
      <Avatar name="Bob Johnson" />
    </div>
  ),
};

export const WithImage: StoryObj = {
  render: () => (
    <div style={{ display: "flex", gap: "16px", alignItems: "center" }}>
      <Avatar
        src="https://images.unsplash.com/photo-1472099645785-5658abf4ff4e?w=100&h=100&fit=crop&crop=face"
        name="John Doe"
      />
      <Avatar
        src="https://images.unsplash.com/photo-1494790108755-2616b612b786?w=100&h=100&fit=crop&crop=face"
        name="Jane Smith"
      />
    </div>
  ),
};

export const Sizes: StoryObj = {
  render: () => (
    <div style={{ display: "flex", gap: "16px", alignItems: "center" }}>
      <Avatar name="XS" size="xs" />
      <Avatar name="SM" size="sm" />
      <Avatar name="MD" size="md" />
      <Avatar name="LG" size="lg" />
      <Avatar name="XL" size="xl" />
      <Avatar name="2XL" size="2xl" />
    </div>
  ),
};

export const Shapes: StoryObj = {
  render: () => (
    <div style={{ display: "flex", gap: "16px", alignItems: "center" }}>
      <Avatar name="Circle" shape="circle" />
      <Avatar name="Square" shape="square" />
    </div>
  ),
};

export const Fallback: StoryObj = {
  render: () => (
    <div style={{ display: "flex", gap: "16px", alignItems: "center" }}>
      <Avatar name="No Image" />
      <Avatar />
    </div>
  ),
};
