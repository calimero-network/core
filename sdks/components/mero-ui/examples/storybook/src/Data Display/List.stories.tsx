import type { Meta, StoryObj } from "@storybook/react-vite";
import { List } from "@calimero-network/mero-ui";
import { cssVariables } from "@calimero-network/mero-tokens";

const withTokens = (Story: any) => (
  <>
    <style>{cssVariables}</style>
    <Story />
  </>
);

const meta: Meta<typeof List> = {
  title: "Data Display/List",
  component: List,
  decorators: [withTokens],
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
};

export default meta;

type Story = StoryObj<typeof List>;

const items = [
  { id: 1, content: "First item", hint: "Hint A" },
  { id: 2, content: "Second item", hint: "Hint B" },
  { id: 3, content: "Third item", hint: "Hint C" },
];

export const Solid: Story = {
  render: () => (
    <div style={{ width: 360 }}>
      <List items={items} variant="solid" />
    </div>
  ),
};

export const GhostWithDividers: Story = {
  render: () => (
    <div style={{ width: 360 }}>
      <List items={items} variant="ghost" divider />
    </div>
  ),
};
