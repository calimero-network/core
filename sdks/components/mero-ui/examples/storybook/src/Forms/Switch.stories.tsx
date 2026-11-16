import type { Meta, StoryObj } from "@storybook/react-vite";
import { Switch } from "@calimero-network/mero-ui";

const meta: Meta<typeof Switch> = {
  title: "Forms/Switch",
  component: Switch,
  parameters: { layout: "centered" },
};

export default meta;

type Story = StoryObj<typeof Switch>;

export const Default: Story = {
  args: { label: "Enable notifications" },
};

export const WithDescription: Story = {
  args: {
    label: "Dark mode",
    description: "Switch between light and dark themes",
  },
};

export const Checked: Story = {
  args: { label: "Auto-save", defaultChecked: true },
};

export const Disabled: Story = {
  args: { label: "Disabled switch", disabled: true },
};

export const Small: Story = {
  args: { label: "Small switch", size: "small" },
};

export const Large: Story = {
  args: { label: "Large switch", size: "large" },
};
