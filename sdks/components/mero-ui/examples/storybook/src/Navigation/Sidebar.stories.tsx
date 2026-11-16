import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  Sidebar,
  SidebarHeader,
  SidebarFooter,
  SidebarBrand,
  SidebarUser,
  SidebarItem,
  SidebarGroup,
} from "@calimero-network/mero-ui";
import { useState } from "react";

const meta: Meta<typeof Sidebar> = {
  title: "Navigation/Sidebar",
  component: Sidebar,
  parameters: {
    layout: "fullscreen",
    docs: {
      description: {
        component:
          "A flexible sidebar navigation component with support for collapsible groups, nested items, and various display modes.",
      },
    },
  },
  argTypes: {
    groups: {
      description: "Array of navigation groups containing items",
      control: "object",
    },
    activeItemId: {
      description: "Currently active item ID",
      control: "text",
    },
    onItemClick: {
      description: "Callback when an item is clicked",
      action: "item clicked",
    },
    variant: {
      description: "Sidebar visual variant",
      control: "select",
      options: ["default", "minimal", "compact"],
    },
    size: {
      description: "Sidebar size",
      control: "select",
      options: ["sm", "md", "lg"],
    },
    collapsed: {
      description: "Whether sidebar is collapsed",
      control: "boolean",
    },
    collapsible: {
      description: "Whether sidebar can be toggled",
      control: "boolean",
    },
    position: {
      description: "Sidebar position",
      control: "select",
      options: ["left", "right"],
    },
    fixed: {
      description: "Whether sidebar is fixed positioned",
      control: "boolean",
    },
    showGroupLabelsWhenCollapsed: {
      description: "Show group labels when collapsed",
      control: "boolean",
    },
    showTooltipsWhenCollapsed: {
      description: "Show tooltips when collapsed",
      control: "boolean",
    },
  },
};

export default meta;
type Story = StoryObj<typeof Sidebar>;

// Sample navigation data
const sampleGroups: SidebarGroup[] = [
  {
    id: "main",
    label: "Main",
    items: [
      {
        id: "dashboard",
        label: "Dashboard",
        icon: "home",
        href: "/dashboard",
      },
      {
        id: "analytics",
        label: "Analytics",
        icon: "star",
        href: "/analytics",
        badge: "New",
      },
      {
        id: "projects",
        label: "Projects",
        icon: "edit",
        href: "/projects",
        children: [
          {
            id: "project-list",
            label: "All Projects",
            href: "/projects",
          },
          {
            id: "project-create",
            label: "Create Project",
            href: "/projects/create",
          },
          {
            id: "project-templates",
            label: "Templates",
            href: "/projects/templates",
          },
        ],
      },
    ],
  },
  {
    id: "tools",
    label: "Tools",
    items: [
      {
        id: "calendar",
        label: "Calendar",
        icon: "calendar",
        href: "/calendar",
      },
      {
        id: "messages",
        label: "Messages",
        icon: "mail",
        href: "/messages",
        badge: 5,
      },
      {
        id: "files",
        label: "Files",
        icon: "upload",
        href: "/files",
      },
    ],
  },
  {
    id: "settings",
    label: "Settings",
    items: [
      {
        id: "profile",
        label: "Profile",
        icon: "user",
        href: "/profile",
      },
      {
        id: "settings",
        label: "Settings",
        icon: "settings",
        href: "/settings",
      },
      {
        id: "help",
        label: "Help & Support",
        icon: "info",
        href: "/help",
        external: true,
      },
    ],
  },
];

const minimalGroups: SidebarGroup[] = [
  {
    id: "main",
    items: [
      { id: "home", label: "Home", icon: "home", href: "/" },
      { id: "search", label: "Search", icon: "search", href: "/search" },
      {
        id: "notifications",
        label: "Notifications",
        icon: "star",
        href: "/notifications",
        badge: 3,
      },
    ],
  },
];

const complexGroups: SidebarGroup[] = [
  {
    id: "workspace",
    label: "Workspace",
    collapsible: true,
    defaultCollapsed: false,
    items: [
      {
        id: "overview",
        label: "Overview",
        icon: "home",
        href: "/overview",
      },
      {
        id: "team",
        label: "Team",
        icon: "user",
        children: [
          {
            id: "team-members",
            label: "Members",
            href: "/team/members",
          },
          {
            id: "team-roles",
            label: "Roles & Permissions",
            href: "/team/roles",
          },
          {
            id: "team-invites",
            label: "Invitations",
            href: "/team/invites",
            badge: 2,
          },
        ],
      },
      {
        id: "projects",
        label: "Projects",
        icon: "edit",
        children: [
          {
            id: "project-active",
            label: "Active Projects",
            href: "/projects/active",
            badge: 8,
          },
          {
            id: "project-archived",
            label: "Archived",
            href: "/projects/archived",
          },
          {
            id: "project-templates",
            label: "Templates",
            href: "/projects/templates",
          },
        ],
      },
    ],
  },
  {
    id: "tools",
    label: "Tools",
    collapsible: true,
    defaultCollapsed: true,
    items: [
      {
        id: "calendar",
        label: "Calendar",
        icon: "calendar",
        href: "/calendar",
      },
      {
        id: "files",
        label: "File Manager",
        icon: "upload",
        href: "/files",
      },
      {
        id: "analytics",
        label: "Analytics",
        icon: "star",
        href: "/analytics",
      },
    ],
  },
  {
    id: "account",
    label: "Account",
    items: [
      {
        id: "profile",
        label: "Profile Settings",
        icon: "user",
        href: "/profile",
      },
      {
        id: "billing",
        label: "Billing",
        icon: "check-circle",
        href: "/billing",
      },
      {
        id: "support",
        label: "Support",
        icon: "info",
        href: "/support",
        external: true,
      },
    ],
  },
];

// Interactive wrapper component
const SidebarWrapper = (args: any) => {
  const [activeItemId, setActiveItemId] = useState(
    args.activeItemId || "dashboard",
  );
  const [collapsed, setCollapsed] = useState(args.collapsed || false);

  const handleItemClick = (item: SidebarItem) => {
    setActiveItemId(item.id);
    args.onItemClick?.(item);
  };

  const handleCollapseChange = (newCollapsed: boolean) => {
    setCollapsed(newCollapsed);
    args.onCollapseChange?.(newCollapsed);
  };

  return (
    <div style={{ display: "flex", height: "100vh" }}>
      <Sidebar
        {...args}
        activeItemId={activeItemId}
        collapsed={collapsed}
        onItemClick={handleItemClick}
        onCollapseChange={handleCollapseChange}
      />
      <div
        style={{
          flex: 1,
          padding: "24px",
          backgroundColor: "#0F0F0F",
          color: "#FFFFFF",
        }}
      >
        <h2>Main Content Area</h2>
        <p>
          This is the main content area. The sidebar can be toggled and items
          can be clicked.
        </p>
        <p>
          Active Item: <strong>{activeItemId}</strong>
        </p>
        <p>
          Sidebar Collapsed: <strong>{collapsed ? "Yes" : "No"}</strong>
        </p>
      </div>
    </div>
  );
};

// Basic sidebar
export const Default: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: sampleGroups,
    variant: "default",
    size: "md",
    collapsible: true,
    fixed: false,
  },
};

// Collapsed sidebar
export const Collapsed: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: sampleGroups,
    variant: "default",
    size: "md",
    collapsed: true,
    collapsible: true,
    showTooltipsWhenCollapsed: true,
  },
};

// Minimal variant
export const Minimal: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: minimalGroups,
    variant: "minimal",
    size: "md",
    collapsible: true,
  },
};

// Compact variant
export const Compact: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: sampleGroups,
    variant: "compact",
    size: "sm",
    collapsible: true,
  },
};

// Different sizes
export const Small: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: sampleGroups,
    variant: "default",
    size: "sm",
    collapsible: true,
  },
};

export const Large: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: sampleGroups,
    variant: "default",
    size: "lg",
    collapsible: true,
  },
};

// Right positioned sidebar
export const RightPositioned: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: sampleGroups,
    variant: "default",
    size: "md",
    position: "right",
    collapsible: true,
  },
};

// Fixed sidebar
export const Fixed: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: sampleGroups,
    variant: "default",
    size: "md",
    fixed: true,
    collapsible: true,
  },
};

// With header and footer
export const WithHeaderFooter: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: sampleGroups,
    variant: "default",
    size: "md",
    collapsible: true,
    header: (
      <SidebarHeader>
        <SidebarBrand>MyApp</SidebarBrand>
      </SidebarHeader>
    ),
    footer: (
      <SidebarFooter>
        <SidebarUser
          name="John Doe"
          email="john@example.com"
          avatar="https://images.unsplash.com/photo-1472099645785-5658abf4ff4e?w=32&h=32&fit=crop&crop=face"
        />
      </SidebarFooter>
    ),
  },
};

// Complex navigation with nested items
export const ComplexNavigation: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: complexGroups,
    variant: "default",
    size: "md",
    collapsible: true,
    activeItemId: "overview",
  },
};

// Disabled items
export const WithDisabledItems: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: [
      {
        id: "main",
        label: "Main",
        items: [
          {
            id: "dashboard",
            label: "Dashboard",
            icon: "home",
            href: "/dashboard",
          },
          {
            id: "analytics",
            label: "Analytics",
            icon: "star",
            href: "/analytics",
            disabled: true,
          },
          {
            id: "reports",
            label: "Reports",
            icon: "edit",
            href: "/reports",
            disabled: true,
          },
        ],
      },
    ],
    variant: "default",
    size: "md",
    collapsible: true,
  },
};

// External links
export const WithExternalLinks: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: [
      {
        id: "main",
        label: "Main",
        items: [
          {
            id: "dashboard",
            label: "Dashboard",
            icon: "home",
            href: "/dashboard",
          },
          {
            id: "documentation",
            label: "Documentation",
            icon: "external-link",
            href: "https://docs.example.com",
            external: true,
          },
          {
            id: "support",
            label: "Support",
            icon: "info",
            href: "https://support.example.com",
            external: true,
          },
        ],
      },
    ],
    variant: "default",
    size: "md",
    collapsible: true,
  },
};

// Non-collapsible sidebar
export const NonCollapsible: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: sampleGroups,
    variant: "default",
    size: "md",
    collapsible: false,
  },
};

// With badges and indicators
export const WithBadges: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: [
      {
        id: "main",
        label: "Main",
        items: [
          {
            id: "dashboard",
            label: "Dashboard",
            icon: "home",
            href: "/dashboard",
          },
          {
            id: "messages",
            label: "Messages",
            icon: "mail",
            href: "/messages",
            badge: 12,
          },
          {
            id: "notifications",
            label: "Notifications",
            icon: "star",
            href: "/notifications",
            badge: "New",
          },
          {
            id: "tasks",
            label: "Tasks",
            icon: "check-circle",
            href: "/tasks",
            badge: 99,
          },
        ],
      },
    ],
    variant: "default",
    size: "md",
    collapsible: true,
  },
};

// Collapsed with tooltips
export const CollapsedWithTooltips: Story = {
  render: (args) => <SidebarWrapper {...args} />,
  args: {
    groups: sampleGroups,
    variant: "default",
    size: "md",
    collapsed: true,
    collapsible: true,
    showTooltipsWhenCollapsed: true,
  },
};

// All variants showcase
export const AllVariants: Story = {
  render: () => (
    <div style={{ display: "flex", height: "100vh" }}>
      <Sidebar
        groups={sampleGroups}
        variant="default"
        size="sm"
        collapsible={false}
        style={{ borderRight: "1px solid #404040" }}
      />
      <Sidebar
        groups={sampleGroups}
        variant="minimal"
        size="sm"
        collapsible={false}
        style={{ borderRight: "1px solid #404040" }}
      />
      <Sidebar
        groups={sampleGroups}
        variant="compact"
        size="sm"
        collapsible={false}
        style={{ borderRight: "1px solid #404040" }}
      />
      <div
        style={{
          flex: 1,
          padding: "24px",
          backgroundColor: "#0F0F0F",
          color: "#FFFFFF",
        }}
      >
        <h2>All Variants Showcase</h2>
        <p>From left to right: Default, Minimal, Compact</p>
      </div>
    </div>
  ),
  parameters: {
    docs: {
      description: {
        story: "Showcase of all sidebar variants side by side.",
      },
    },
  },
};
