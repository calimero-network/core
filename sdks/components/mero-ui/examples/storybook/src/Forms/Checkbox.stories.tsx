import type { Meta, StoryObj } from "@storybook/react-vite";
import { Checkbox } from "@calimero-network/mero-ui";

const meta: Meta<typeof Checkbox> = {
  title: "Forms/Checkbox",
  component: Checkbox,
  parameters: { layout: "centered" },
};

export default meta;

type Story = StoryObj<typeof Checkbox>;

export const Default: Story = {
  args: { label: "Accept terms and conditions" },
};

export const WithDescription: Story = {
  args: {
    label: "Enable notifications",
    description: "Receive updates about your account activity",
  },
};

export const Checked: Story = {
  args: { label: "Remember me", defaultChecked: true },
};

export const Disabled: Story = {
  args: { label: "Disabled option", disabled: true },
};

export const DisabledChecked: Story = {
  args: { label: "Disabled checked", disabled: true, defaultChecked: true },
};
