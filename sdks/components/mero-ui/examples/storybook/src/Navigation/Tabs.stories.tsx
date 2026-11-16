import type { Meta, StoryObj } from "@storybook/react-vite";
import { Tabs, TabPanel } from "@calimero-network/mero-ui";
import React from "react";

const meta: Meta<typeof Tabs> = {
  title: "Navigation/Tabs",
  component: Tabs,
  parameters: { layout: "centered" },
};

export default meta;

type Story = StoryObj<typeof Tabs>;

export const Uncontrolled: Story = {
  render: () => {
    const tabs = [
      { id: "overview", label: "Overview" },
      { id: "details", label: "Details" },
      { id: "settings", label: "Settings" },
    ];
    const [active, setActive] = React.useState("overview");
    return (
      <div style={{ width: 520 }}>
        <Tabs tabs={tabs} value={active} onValueChange={setActive} />
        <TabPanel when="overview" active={active}>
          Overview content
        </TabPanel>
        <TabPanel when="details" active={active}>
          Details content
        </TabPanel>
        <TabPanel when="settings" active={active}>
          Settings content
        </TabPanel>
      </div>
    );
  },
};
