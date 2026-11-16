import type { Meta, StoryObj } from "@storybook/react-vite";
import { Radio, RadioGroup } from "@calimero-network/mero-ui";
import React from "react";

const meta: Meta<typeof Radio> = {
  title: "Forms/Radio",
  component: Radio,
  parameters: { layout: "centered" },
};

export default meta;

type Story = StoryObj<typeof Radio>;

export const Default: Story = {
  args: { label: "Option 1", value: "option1" },
};

export const WithDescription: Story = {
  args: {
    label: "Premium plan",
    description: "Includes all features and priority support",
    value: "premium",
  },
};

export const Checked: Story = {
  args: { label: "Selected option", value: "selected", defaultChecked: true },
};

export const Disabled: Story = {
  args: { label: "Disabled option", value: "disabled", disabled: true },
};

export const Group: Story = {
  render: () => {
    const [value, setValue] = React.useState("option1");
    return (
      <RadioGroup value={value} onChange={setValue} name="example">
        <Radio label="Option 1" value="option1" />
        <Radio label="Option 2" value="option2" />
        <Radio label="Option 3" value="option3" />
      </RadioGroup>
    );
  },
};
