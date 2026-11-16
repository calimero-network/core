import type { Meta, StoryObj } from "@storybook/react-vite";
import { Breadcrumbs } from "@calimero-network/mero-ui";

const meta: Meta<typeof Breadcrumbs> = {
  title: "Navigation/Breadcrumbs",
  component: Breadcrumbs,
  parameters: { layout: "centered" },
};

export default meta;

type Story = StoryObj<typeof Breadcrumbs>;

export const Basic: Story = {
  args: {
    items: [
      { label: "Home", href: "#" },
      { label: "Docs", href: "#" },
      { label: "Guides", href: "#" },
      { label: "Getting Started" },
    ],
  },
};
