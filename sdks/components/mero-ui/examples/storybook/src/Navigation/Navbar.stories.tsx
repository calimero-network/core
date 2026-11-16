import React from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  Navbar,
  NavbarBrand,
  NavbarItem,
  NavbarMenu,
  NavbarToggle,
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

const meta: Meta<typeof Navbar> = {
  title: "Navigation/Navbar",
  component: Navbar,
  decorators: [withTokens],
  parameters: {
    layout: "fullscreen",
  },
  tags: ["autodocs"],
  argTypes: {
    variant: {
      control: { type: "select" },
      options: ["default", "minimal", "elevated"],
      description: "Navbar variant style",
    },
    size: {
      control: { type: "select" },
      options: ["sm", "md", "lg"],
      description: "Navbar size",
    },
    fixed: {
      control: { type: "boolean" },
      description: "Whether the navbar is fixed",
    },
  },
};

export default meta;

type Story = StoryObj<typeof Navbar>;

// Icon components for demo
const HomeIcon = () => (
  <svg
    width="20"
    height="20"
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
    width="20"
    height="20"
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
    width="20"
    height="20"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
  >
    <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
    <circle cx="12" cy="7" r="4" />
  </svg>
);

const BellIcon = () => (
  <svg
    width="20"
    height="20"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
  >
    <path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9" />
    <path d="M13.73 21a2 2 0 0 1-3.46 0" />
  </svg>
);

const LogoIcon = () => (
  <div
    style={{
      width: "32px",
      height: "32px",
      backgroundColor: "#3B82F6",
      borderRadius: "6px",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      color: "white",
      fontWeight: "bold",
      fontSize: "16px",
    }}
  >
    C
  </div>
);

export const Basic: Story = {
  render: () => (
    <Navbar>
      <NavbarBrand logo={<LogoIcon />} text="Calimero" />
      <NavbarMenu>
        <NavbarItem icon={<HomeIcon />}>Home</NavbarItem>
        <NavbarItem icon={<SettingsIcon />}>Settings</NavbarItem>
        <NavbarItem icon={<UserIcon />}>Profile</NavbarItem>
      </NavbarMenu>
    </Navbar>
  ),
};

export const WithActiveState: Story = {
  render: () => {
    const [activeItem, setActiveItem] = React.useState("home");

    return (
      <Navbar>
        <NavbarBrand logo={<LogoIcon />} text="Calimero" />
        <NavbarMenu>
          <NavbarItem
            icon={<HomeIcon />}
            active={activeItem === "home"}
            onClick={() => setActiveItem("home")}
          >
            Home
          </NavbarItem>
          <NavbarItem
            icon={<SettingsIcon />}
            active={activeItem === "settings"}
            onClick={() => setActiveItem("settings")}
          >
            Settings
          </NavbarItem>
          <NavbarItem
            icon={<UserIcon />}
            active={activeItem === "profile"}
            onClick={() => setActiveItem("profile")}
          >
            Profile
          </NavbarItem>
        </NavbarMenu>
      </Navbar>
    );
  },
};

export const Variants: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <div>
        <h3 style={{ color: "white", marginBottom: "8px", fontSize: "14px" }}>
          Default
        </h3>
        <Navbar variant="default">
          <NavbarBrand logo={<LogoIcon />} text="Calimero" />
          <NavbarMenu>
            <NavbarItem icon={<HomeIcon />}>Home</NavbarItem>
            <NavbarItem icon={<SettingsIcon />}>Settings</NavbarItem>
          </NavbarMenu>
        </Navbar>
      </div>

      <div>
        <h3 style={{ color: "white", marginBottom: "8px", fontSize: "14px" }}>
          Minimal
        </h3>
        <Navbar variant="minimal">
          <NavbarBrand logo={<LogoIcon />} text="Calimero" />
          <NavbarMenu>
            <NavbarItem icon={<HomeIcon />}>Home</NavbarItem>
            <NavbarItem icon={<SettingsIcon />}>Settings</NavbarItem>
          </NavbarMenu>
        </Navbar>
      </div>

      <div>
        <h3 style={{ color: "white", marginBottom: "8px", fontSize: "14px" }}>
          Elevated
        </h3>
        <Navbar variant="elevated">
          <NavbarBrand logo={<LogoIcon />} text="Calimero" />
          <NavbarMenu>
            <NavbarItem icon={<HomeIcon />}>Home</NavbarItem>
            <NavbarItem icon={<SettingsIcon />}>Settings</NavbarItem>
          </NavbarMenu>
        </Navbar>
      </div>
    </div>
  ),
};

export const Sizes: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <div>
        <h3 style={{ color: "white", marginBottom: "8px", fontSize: "14px" }}>
          Small
        </h3>
        <Navbar size="sm">
          <NavbarBrand logo={<LogoIcon />} text="Calimero" />
          <NavbarMenu>
            <NavbarItem icon={<HomeIcon />}>Home</NavbarItem>
            <NavbarItem icon={<SettingsIcon />}>Settings</NavbarItem>
          </NavbarMenu>
        </Navbar>
      </div>

      <div>
        <h3 style={{ color: "white", marginBottom: "8px", fontSize: "14px" }}>
          Medium
        </h3>
        <Navbar size="md">
          <NavbarBrand logo={<LogoIcon />} text="Calimero" />
          <NavbarMenu>
            <NavbarItem icon={<HomeIcon />}>Home</NavbarItem>
            <NavbarItem icon={<SettingsIcon />}>Settings</NavbarItem>
          </NavbarMenu>
        </Navbar>
      </div>

      <div>
        <h3 style={{ color: "white", marginBottom: "8px", fontSize: "14px" }}>
          Large
        </h3>
        <Navbar size="lg">
          <NavbarBrand logo={<LogoIcon />} text="Calimero" />
          <NavbarMenu>
            <NavbarItem icon={<HomeIcon />}>Home</NavbarItem>
            <NavbarItem icon={<SettingsIcon />}>Settings</NavbarItem>
          </NavbarMenu>
        </Navbar>
      </div>
    </div>
  ),
};

export const WithDropdown: Story = {
  render: () => {
    const [showMenu, setShowMenu] = React.useState(false);

    return (
      <div style={{ position: "relative" }}>
        <Navbar>
          <NavbarBrand logo={<LogoIcon />} text="Calimero" />
          <NavbarMenu align="right">
            <NavbarItem icon={<HomeIcon />}>Home</NavbarItem>
            <NavbarItem icon={<SettingsIcon />}>Settings</NavbarItem>
            <NavbarItem icon={<BellIcon />}>Notifications</NavbarItem>
            <NavbarItem
              icon={<UserIcon />}
              onClick={() => setShowMenu(!showMenu)}
            >
              Account
            </NavbarItem>
          </NavbarMenu>
        </Navbar>

        {showMenu && (
          <div
            style={{
              position: "absolute",
              top: "100%",
              right: "24px",
              zIndex: 1000,
            }}
          >
            <Menu>
              <MenuGroup label="Account">
                <MenuItem icon={<UserIcon />}>Profile</MenuItem>
                <MenuItem icon={<SettingsIcon />}>Settings</MenuItem>
                <MenuItem>Billing</MenuItem>
              </MenuGroup>
              <MenuDivider />
              <MenuGroup>
                <MenuItem>Sign Out</MenuItem>
              </MenuGroup>
            </Menu>
          </div>
        )}
      </div>
    );
  },
};

export const MobileWithToggle: Story = {
  render: () => {
    const [isMenuOpen, setIsMenuOpen] = React.useState(false);

    return (
      <div>
        <Navbar>
          <NavbarBrand logo={<LogoIcon />} text="Calimero" />
          <NavbarMenu align="right">
            <NavbarToggle
              isOpen={isMenuOpen}
              onToggle={() => setIsMenuOpen(!isMenuOpen)}
            />
          </NavbarMenu>
        </Navbar>

        {isMenuOpen && (
          <div
            style={{
              backgroundColor: "#1A1A1A",
              borderBottom: "1px solid #404040",
              padding: "16px 24px",
            }}
          >
            <div
              style={{ display: "flex", flexDirection: "column", gap: "8px" }}
            >
              <NavbarItem icon={<HomeIcon />}>Home</NavbarItem>
              <NavbarItem icon={<SettingsIcon />}>Settings</NavbarItem>
              <NavbarItem icon={<UserIcon />}>Profile</NavbarItem>
              <NavbarItem icon={<BellIcon />}>Notifications</NavbarItem>
            </div>
          </div>
        )}
      </div>
    );
  },
};

export const ComplexLayout: Story = {
  render: () => (
    <Navbar variant="elevated" size="lg">
      <NavbarBrand logo={<LogoIcon />} text="Calimero Dashboard" />
      <NavbarMenu align="center">
        <NavbarItem icon={<HomeIcon />}>Dashboard</NavbarItem>
        <NavbarItem icon={<UserIcon />}>Users</NavbarItem>
        <NavbarItem icon={<SettingsIcon />}>Settings</NavbarItem>
      </NavbarMenu>
      <NavbarMenu align="right">
        <NavbarItem icon={<BellIcon />}>Notifications</NavbarItem>
        <NavbarItem icon={<UserIcon />}>Profile</NavbarItem>
      </NavbarMenu>
    </Navbar>
  ),
};

export const FixedNavbar: Story = {
  render: () => (
    <div
      style={{
        height: "200vh",
        background: "linear-gradient(45deg, #1A1A1A, #2A2A2A)",
      }}
    >
      <Navbar fixed variant="elevated">
        <NavbarBrand logo={<LogoIcon />} text="Fixed Navbar" />
        <NavbarMenu>
          <NavbarItem icon={<HomeIcon />}>Home</NavbarItem>
          <NavbarItem icon={<SettingsIcon />}>Settings</NavbarItem>
          <NavbarItem icon={<UserIcon />}>Profile</NavbarItem>
        </NavbarMenu>
      </Navbar>

      <div
        style={{
          padding: "80px 24px 24px 24px",
          color: "white",
          textAlign: "center",
        }}
      >
        <h1>Scroll to see the fixed navbar in action!</h1>
        <p>This content is here to demonstrate the fixed navbar behavior.</p>
      </div>
    </div>
  ),
};

export const WithControls: Story = {
  argTypes: {
    variant: {
      control: { type: "select" },
      options: ["default", "minimal", "elevated"],
    },
    size: {
      control: { type: "select" },
      options: ["sm", "md", "lg"],
    },
    fixed: {
      control: { type: "boolean" },
    },
  },
  args: {
    variant: "default",
    size: "md",
    fixed: false,
  },
  render: (args) => (
    <Navbar {...args}>
      <NavbarBrand logo={<LogoIcon />} text="Calimero" />
      <NavbarMenu>
        <NavbarItem icon={<HomeIcon />}>Home</NavbarItem>
        <NavbarItem icon={<SettingsIcon />}>Settings</NavbarItem>
        <NavbarItem icon={<UserIcon />}>Profile</NavbarItem>
      </NavbarMenu>
    </Navbar>
  ),
};
