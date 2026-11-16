import React from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  Menu,
  MenuItem,
  MenuGroup,
  MenuDivider,
} from "@calimero-network/mero-ui";
import { cssVariables } from "@calimero-network/mero-tokens";

const withTokens = (Story: any) => (
  <>
    <style>{cssVariables}</style>
    <Story />
  </>
);

const meta: Meta<typeof Menu> = {
  title: "Navigation/Menu",
  component: Menu,
  decorators: [withTokens],
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
  argTypes: {
    variant: {
      control: { type: "select" },
      options: ["default", "compact"],
      description: "Menu variant style",
    },
    size: {
      control: { type: "select" },
      options: ["sm", "md", "lg"],
      description: "Menu size",
    },
  },
};

export default meta;

type Story = StoryObj<typeof Menu>;

// Simple icon components for demo
const HomeIcon = () => (
  <svg
    width="16"
    height="16"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
  >
    <path d="m3 9 9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
    <polyline points="9,22 9,12 15,12 15,22" />
  </svg>
);

const SettingsIcon = () => (
  <svg
    width="16"
    height="16"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
  >
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1 1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
  </svg>
);

const UserIcon = () => (
  <svg
    width="16"
    height="16"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
  >
    <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
    <circle cx="12" cy="7" r="4" />
  </svg>
);

const ChevronRightIcon = () => (
  <svg
    width="16"
    height="16"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
  >
    <polyline points="9,18 15,12 9,6" />
  </svg>
);

export const Basic: Story = {
  args: {
    variant: "compact",
  },

  render: () => (
    <Menu>
      <MenuItem icon={<HomeIcon />}>Home</MenuItem>
      <MenuItem icon={<SettingsIcon />}>Settings</MenuItem>
      <MenuItem icon={<UserIcon />}>Profile</MenuItem>
    </Menu>
  ),
};

export const WithGroups: Story = {
  render: () => (
    <Menu>
      <MenuGroup label="Navigation">
        <MenuItem icon={<HomeIcon />}>Dashboard</MenuItem>
        <MenuItem icon={<UserIcon />}>Users</MenuItem>
        <MenuItem icon={<SettingsIcon />}>Settings</MenuItem>
      </MenuGroup>
      <MenuDivider />
      <MenuGroup label="Account">
        <MenuItem>Profile</MenuItem>
        <MenuItem>Billing</MenuItem>
        <MenuItem>Sign Out</MenuItem>
      </MenuGroup>
    </Menu>
  ),
};

export const WithStates: Story = {
  render: () => (
    <Menu>
      <MenuItem icon={<HomeIcon />}>Normal Item</MenuItem>
      <MenuItem selected icon={<SettingsIcon />}>
        Selected Item
      </MenuItem>
      <MenuItem disabled icon={<UserIcon />}>
        Disabled Item
      </MenuItem>
      <MenuItem icon={<HomeIcon />} endIcon={<ChevronRightIcon />}>
        With End Icon
      </MenuItem>
    </Menu>
  ),
};

export const Sizes: Story = {
  args: {
    variant: "compact",
  },

  render: () => (
    <div style={{ display: "flex", gap: "24px", alignItems: "flex-start" }}>
      <div>
        <h3 style={{ color: "white", marginBottom: "8px", fontSize: "14px" }}>
          Small
        </h3>
        <Menu size="sm">
          <MenuItem icon={<HomeIcon />}>Home</MenuItem>
          <MenuItem icon={<SettingsIcon />}>Settings</MenuItem>
          <MenuItem icon={<UserIcon />}>Profile</MenuItem>
        </Menu>
      </div>

      <div>
        <h3 style={{ color: "white", marginBottom: "8px", fontSize: "14px" }}>
          Medium
        </h3>
        <Menu size="md">
          <MenuItem icon={<HomeIcon />}>Home</MenuItem>
          <MenuItem icon={<SettingsIcon />}>Settings</MenuItem>
          <MenuItem icon={<UserIcon />}>Profile</MenuItem>
        </Menu>
      </div>

      <div>
        <h3 style={{ color: "white", marginBottom: "8px", fontSize: "14px" }}>
          Large
        </h3>
        <Menu size="lg">
          <MenuItem icon={<HomeIcon />}>Home</MenuItem>
          <MenuItem icon={<SettingsIcon />}>Settings</MenuItem>
          <MenuItem icon={<UserIcon />}>Profile</MenuItem>
        </Menu>
      </div>
    </div>
  ),
};

export const Interactive: Story = {
  render: () => {
    const [selectedItem, setSelectedItem] = React.useState("dashboard");

    return (
      <Menu>
        <MenuGroup label="Main Navigation">
          <MenuItem
            icon={<HomeIcon />}
            selected={selectedItem === "dashboard"}
            onClick={() => setSelectedItem("dashboard")}
          >
            Dashboard
          </MenuItem>
          <MenuItem
            icon={<UserIcon />}
            selected={selectedItem === "users"}
            onClick={() => setSelectedItem("users")}
          >
            Users
          </MenuItem>
          <MenuItem
            icon={<SettingsIcon />}
            selected={selectedItem === "settings"}
            onClick={() => setSelectedItem("settings")}
          >
            Settings
          </MenuItem>
        </MenuGroup>
        <MenuDivider />
        <MenuGroup label="Account">
          <MenuItem onClick={() => alert("Profile clicked!")}>Profile</MenuItem>
          <MenuItem onClick={() => alert("Billing clicked!")}>Billing</MenuItem>
          <MenuItem onClick={() => alert("Sign out clicked!")}>
            Sign Out
          </MenuItem>
        </MenuGroup>
      </Menu>
    );
  },
};

export const ComplexMenu: Story = {
  render: () => (
    <Menu size="lg">
      <MenuGroup label="Workspace">
        <MenuItem icon={<HomeIcon />} endIcon={<ChevronRightIcon />}>
          Overview
        </MenuItem>
        <MenuItem icon={<UserIcon />} endIcon={<ChevronRightIcon />}>
          Team Members
        </MenuItem>
        <MenuItem icon={<SettingsIcon />}>Project Settings</MenuItem>
      </MenuGroup>

      <MenuDivider />

      <MenuGroup label="Tools">
        <MenuItem>Analytics</MenuItem>
        <MenuItem>Reports</MenuItem>
        <MenuItem>Integrations</MenuItem>
      </MenuGroup>

      <MenuDivider />

      <MenuGroup label="Account">
        <MenuItem>Profile Settings</MenuItem>
        <MenuItem>Billing & Plans</MenuItem>
        <MenuItem disabled>Upgrade Plan</MenuItem>
        <MenuItem>Sign Out</MenuItem>
      </MenuGroup>
    </Menu>
  ),
};

export const WithControls: Story = {
  argTypes: {
    variant: {
      control: { type: "select" },
      options: ["default", "compact"],
    },
    size: {
      control: { type: "select" },
      options: ["sm", "md", "lg"],
    },
  },
  args: {
    variant: "default",
    size: "md",
  },
  render: (args) => (
    <Menu {...args}>
      <MenuItem icon={<HomeIcon />}>Home</MenuItem>
      <MenuItem icon={<SettingsIcon />}>Settings</MenuItem>
      <MenuItem icon={<UserIcon />}>Profile</MenuItem>
    </Menu>
  ),
};
