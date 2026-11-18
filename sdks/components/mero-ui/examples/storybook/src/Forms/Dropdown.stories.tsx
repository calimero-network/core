import type { Meta, StoryObj } from "@storybook/react-vite";
import { Dropdown, Icon } from "@calimero-network/mero-ui";
import React, { useState } from "react";

const meta: Meta<typeof Dropdown> = {
  title: "Forms/Dropdown",
  component: Dropdown,
  parameters: {
    layout: "padded",
  },
  tags: ["autodocs"],
};

export default meta;
type Story = StoryObj<typeof Dropdown>;

const basicItems = [
  { value: "option1", label: "Option 1" },
  { value: "option2", label: "Option 2" },
  { value: "option3", label: "Option 3" },
  { value: "option4", label: "Option 4" },
];

const itemsWithIcons = [
  {
    value: "home",
    label: "Home",
    icon: <Icon name="home" size="sm" color="current" />,
  },
  {
    value: "settings",
    label: "Settings",
    icon: <Icon name="settings" size="sm" color="current" />,
  },
  {
    value: "user",
    label: "Profile",
    icon: <Icon name="user" size="sm" color="current" />,
  },
  {
    value: "logout",
    label: "Logout",
    icon: <Icon name="x" size="sm" color="current" />,
  },
];

const itemsWithDisabled = [
  { value: "option1", label: "Option 1" },
  { value: "option2", label: "Option 2", disabled: true },
  { value: "option3", label: "Option 3" },
  { value: "option4", label: "Option 4", disabled: true },
];

export const Default: Story = {
  render: () => {
    const [selectedValue, setSelectedValue] = useState("");

    return (
      <Dropdown
        items={basicItems}
        value={selectedValue}
        onSelect={(value) => setSelectedValue(value)}
        placeholder="Select an option"
        style={{ width: "200px" }}
      />
    );
  },
};

export const WithIcons: Story = {
  render: () => {
    const [selectedValue, setSelectedValue] = useState("");

    return (
      <Dropdown
        items={itemsWithIcons}
        value={selectedValue}
        onSelect={(value) => setSelectedValue(value)}
        placeholder="Choose an action"
        style={{ width: "200px" }}
      />
    );
  },
};

export const WithDisabledItems: Story = {
  render: () => {
    const [selectedValue, setSelectedValue] = useState("");

    return (
      <Dropdown
        items={itemsWithDisabled}
        value={selectedValue}
        onSelect={(value) => setSelectedValue(value)}
        placeholder="Select an option"
        style={{ width: "200px" }}
      />
    );
  },
};

export const DifferentSizes: Story = {
  render: () => {
    const [smValue, setSmValue] = useState("");
    const [mdValue, setMdValue] = useState("");
    const [lgValue, setLgValue] = useState("");

    return (
      <div style={{ display: "flex", gap: "16px", alignItems: "flex-start" }}>
        <Dropdown
          items={basicItems}
          value={smValue}
          onSelect={setSmValue}
          size="sm"
          placeholder="Small"
          style={{ width: "150px" }}
        />

        <Dropdown
          items={basicItems}
          value={mdValue}
          onSelect={setMdValue}
          size="md"
          placeholder="Medium"
          style={{ width: "150px" }}
        />

        <Dropdown
          items={basicItems}
          value={lgValue}
          onSelect={setLgValue}
          size="lg"
          placeholder="Large"
          style={{ width: "150px" }}
        />
      </div>
    );
  },
};

export const DifferentVariants: Story = {
  render: () => {
    const [defaultValue, setDefaultValue] = useState("");
    const [filledValue, setFilledValue] = useState("");
    const [outlinedValue, setOutlinedValue] = useState("");

    return (
      <div style={{ display: "flex", gap: "16px", alignItems: "flex-start" }}>
        <Dropdown
          items={basicItems}
          value={defaultValue}
          onSelect={setDefaultValue}
          variant="default"
          placeholder="Default"
          style={{ width: "150px" }}
        />

        <Dropdown
          items={basicItems}
          value={filledValue}
          onSelect={setFilledValue}
          variant="filled"
          placeholder="Filled"
          style={{ width: "150px" }}
        />

        <Dropdown
          items={basicItems}
          value={outlinedValue}
          onSelect={setOutlinedValue}
          variant="outlined"
          placeholder="Outlined"
          style={{ width: "150px" }}
        />
      </div>
    );
  },
};

export const Disabled: Story = {
  render: () => (
    <Dropdown
      items={basicItems}
      disabled
      placeholder="Disabled dropdown"
      style={{ width: "200px" }}
    />
  ),
};

export const LongList: Story = {
  render: () => {
    const [selectedValue, setSelectedValue] = useState("");

    const longItems = Array.from({ length: 20 }, (_, i) => ({
      value: `option${i + 1}`,
      label: `Option ${i + 1}`,
    }));

    return (
      <Dropdown
        items={longItems}
        value={selectedValue}
        onSelect={(value) => setSelectedValue(value)}
        placeholder="Select from long list"
        style={{ width: "200px" }}
      />
    );
  },
};
