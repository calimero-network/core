import type { Meta, StoryObj } from "@storybook/react-vite";
import { Input } from "@calimero-network/mero-ui";
import { cssVariables } from "@calimero-network/mero-tokens";

const withTokens = (Story: any) => (
  <>
    <style>{cssVariables}</style>
    <Story />
  </>
);

const meta: Meta<typeof Input> = {
  title: "Forms/Input",
  component: Input,
  decorators: [withTokens],
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
};

export default meta;

type Story = StoryObj<typeof Input>;

export const Basic: Story = {
  render: () => (
    <div style={{ width: 320 }}>
      <Input placeholder="Type something..." />
    </div>
  ),
};

export const States: Story = {
  render: () => (
    <div
      style={{ display: "flex", flexDirection: "column", gap: 16, width: 320 }}
    >
      <Input placeholder="Default" />
      <Input placeholder="Disabled" disabled />
    </div>
  ),
};
