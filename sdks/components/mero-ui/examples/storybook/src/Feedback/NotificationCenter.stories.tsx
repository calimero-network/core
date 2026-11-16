import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  NotificationCenter,
  useNotifications,
} from "@calimero-network/mero-ui";
import { useState, useCallback } from "react";
import { Button } from "@calimero-network/mero-ui";

const meta: Meta<typeof NotificationCenter> = {
  title: "Feedback/NotificationCenter",
  component: NotificationCenter,
  parameters: {
    layout: "fullscreen",
    docs: {
      description: {
        component:
          "A comprehensive notification center for managing and displaying various types of notifications. Supports filtering, grouping, actions, and real-time updates.",
      },
    },
  },
  argTypes: {
    open: {
      control: "boolean",
      description: "Whether the notification center is open",
    },
    statusFilter: {
      control: "select",
      options: ["all", "unread", "read", "archived"],
      description: "Filter notifications by status",
    },
    categoryFilter: {
      control: "select",
      options: [
        "all",
        "system",
        "user",
        "security",
        "feature",
        "marketing",
        "maintenance",
      ],
      description: "Filter notifications by category",
    },
    priorityFilter: {
      control: "select",
      options: ["all", "low", "medium", "high", "urgent"],
      description: "Filter notifications by priority",
    },
    size: {
      control: "select",
      options: ["sm", "md", "lg"],
      description: "Size of the notification center",
    },
    position: {
      control: "select",
      options: ["top-right", "top-left", "bottom-right", "bottom-left"],
      description: "Position of the notification center",
    },
    dropdown: {
      control: "boolean",
      description: "Whether to show as a dropdown",
    },
    groupByDate: {
      control: "boolean",
      description: "Whether to group notifications by date",
    },
    showUnreadCount: {
      control: "boolean",
      description: "Whether to show unread count badge",
    },
    showTimestamps: {
      control: "boolean",
      description: "Whether to show notification timestamps",
    },
    showCategories: {
      control: "boolean",
      description: "Whether to show notification categories",
    },
    showPriorities: {
      control: "boolean",
      description: "Whether to show notification priorities",
    },
  },
};

export default meta;
type Story = StoryObj<typeof NotificationCenter>;

// Sample notifications data
const sampleNotifications = [
  {
    id: "1",
    title: "Welcome to Calimero!",
    description:
      "Your account has been successfully created. Start exploring our features.",
    variant: "success" as const,
    priority: "medium" as const,
    status: "unread" as const,
    category: "user" as const,
    timestamp: new Date(Date.now() - 5 * 60 * 1000), // 5 minutes ago
    actions: [
      {
        label: "Get Started",
        onClick: () => alert("Getting started..."),
        variant: "primary" as const,
      },
      {
        label: "Learn More",
        onClick: () => alert("Learning more..."),
        variant: "secondary" as const,
      },
    ],
  },
  {
    id: "2",
    title: "Security Alert",
    description:
      "A new device has logged into your account from an unrecognized location.",
    variant: "warning" as const,
    priority: "high" as const,
    status: "unread" as const,
    category: "security" as const,
    timestamp: new Date(Date.now() - 2 * 60 * 60 * 1000), // 2 hours ago
    actions: [
      {
        label: "Review Activity",
        onClick: () => alert("Reviewing activity..."),
        variant: "primary" as const,
      },
      {
        label: "Secure Account",
        onClick: () => alert("Securing account..."),
        variant: "warning" as const,
      },
    ],
  },
  {
    id: "3",
    title: "New Feature Available",
    description:
      "Check out our latest dashboard improvements and enhanced analytics.",
    variant: "announcement" as const,
    priority: "medium" as const,
    status: "read" as const,
    category: "feature" as const,
    timestamp: new Date(Date.now() - 24 * 60 * 60 * 1000), // 1 day ago
    actions: [
      {
        label: "Explore Features",
        onClick: () => alert("Exploring features..."),
        variant: "primary" as const,
      },
    ],
  },
  {
    id: "4",
    title: "System Maintenance",
    description:
      "Scheduled maintenance will occur on Sunday, 2:00 AM - 4:00 AM UTC.",
    variant: "info" as const,
    priority: "medium" as const,
    status: "read" as const,
    category: "maintenance" as const,
    timestamp: new Date(Date.now() - 2 * 24 * 60 * 60 * 1000), // 2 days ago
  },
  {
    id: "5",
    title: "Special Offer",
    description: "Get 50% off on all premium features this month only!",
    variant: "promotion" as const,
    priority: "low" as const,
    status: "unread" as const,
    category: "marketing" as const,
    timestamp: new Date(Date.now() - 3 * 24 * 60 * 60 * 1000), // 3 days ago
    actions: [
      {
        label: "Claim Offer",
        onClick: () => alert("Claiming offer..."),
        variant: "primary" as const,
      },
      {
        label: "Dismiss",
        onClick: () => alert("Dismissing..."),
        variant: "secondary" as const,
      },
    ],
  },
  {
    id: "6",
    title: "Payment Failed",
    description:
      "Your subscription payment could not be processed. Please update your payment method.",
    variant: "error" as const,
    priority: "urgent" as const,
    status: "unread" as const,
    category: "system" as const,
    timestamp: new Date(Date.now() - 4 * 24 * 60 * 60 * 1000), // 4 days ago
    actions: [
      {
        label: "Update Payment",
        onClick: () => alert("Updating payment..."),
        variant: "primary" as const,
      },
      {
        label: "Contact Support",
        onClick: () => alert("Contacting support..."),
        variant: "secondary" as const,
      },
    ],
  },
];

// Interactive example component
const InteractiveExample = () => {
  const {
    notifications,
    addNotification,
    markAsRead,
    markAllAsRead,
    archive,
    archiveAll,
    deleteNotification,
    clearAll,
  } = useNotifications();

  const [open, setOpen] = useState(false);

  const handleAddNotification = useCallback(
    (type: string) => {
      const now = new Date();
      switch (type) {
        case "success":
          addNotification({
            title: "Success!",
            description: "Your action was completed successfully.",
            variant: "success",
            priority: "medium",
            category: "system",
          });
          break;
        case "error":
          addNotification({
            title: "Error Occurred",
            description: "Something went wrong. Please try again.",
            variant: "error",
            priority: "high",
            category: "system",
          });
          break;
        case "warning":
          addNotification({
            title: "Warning",
            description: "Please review your settings before proceeding.",
            variant: "warning",
            priority: "medium",
            category: "system",
          });
          break;
        case "info":
          addNotification({
            title: "Information",
            description: "Here is some useful information for you.",
            variant: "info",
            priority: "low",
            category: "system",
          });
          break;
        case "announcement":
          addNotification({
            title: "New Feature",
            description: "Check out our latest feature release!",
            variant: "announcement",
            priority: "medium",
            category: "feature",
            actions: [
              {
                label: "Learn More",
                onClick: () => alert("Learning more..."),
                variant: "primary" as const,
              },
            ],
          });
          break;
        case "promotion":
          addNotification({
            title: "Special Offer",
            description: "Limited time offer - 30% off!",
            variant: "promotion",
            priority: "low",
            category: "marketing",
            actions: [
              {
                label: "Claim Now",
                onClick: () => alert("Claiming offer..."),
                variant: "primary" as const,
              },
            ],
          });
          break;
      }
    },
    [addNotification],
  );

  return (
    <div
      style={{
        padding: "20px",
        minHeight: "100vh",
        background: "var(--color-background-primary)",
      }}
    >
      <div style={{ marginBottom: "20px" }}>
        <h2 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
          Notification Center Demo
        </h2>
        <div
          style={{
            display: "flex",
            gap: "8px",
            flexWrap: "wrap",
            marginBottom: "20px",
          }}
        >
          <Button
            onClick={() => handleAddNotification("success")}
            variant="success"
          >
            Add Success
          </Button>
          <Button
            onClick={() => handleAddNotification("error")}
            variant="error"
          >
            Add Error
          </Button>
          <Button
            onClick={() => handleAddNotification("warning")}
            variant="warning"
          >
            Add Warning
          </Button>
          <Button onClick={() => handleAddNotification("info")} variant="info">
            Add Info
          </Button>
          <Button
            onClick={() => handleAddNotification("announcement")}
            variant="primary"
          >
            Add Announcement
          </Button>
          <Button
            onClick={() => handleAddNotification("promotion")}
            variant="secondary"
          >
            Add Promotion
          </Button>
        </div>
        <div style={{ display: "flex", gap: "8px", flexWrap: "wrap" }}>
          <Button onClick={markAllAsRead} variant="secondary" size="sm">
            Mark All Read
          </Button>
          <Button onClick={archiveAll} variant="secondary" size="sm">
            Archive All
          </Button>
          <Button onClick={clearAll} variant="error" size="sm">
            Clear All
          </Button>
        </div>
      </div>

      <NotificationCenter
        notifications={notifications}
        open={open}
        onToggle={setOpen}
        onNotificationClick={(notification) => {
          console.log("Notification clicked:", notification);
          if (notification.status === "unread") {
            markAsRead(notification.id);
          }
        }}
        onMarkAsRead={markAsRead}
        onMarkAllAsRead={markAllAsRead}
        onArchive={archive}
        onArchiveAll={archiveAll}
        onDelete={deleteNotification}
        onClearAll={clearAll}
        showUnreadCount
        groupByDate
        showTimestamps
        showCategories
        showPriorities
      />
    </div>
  );
};

// Basic examples
export const Default: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    showUnreadCount: true,
    groupByDate: true,
    showTimestamps: true,
  },
};

export const Dropdown: Story = {
  args: {
    notifications: sampleNotifications,
    dropdown: true,
    showUnreadCount: true,
    groupByDate: true,
    showTimestamps: true,
  },
};

export const Small: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    size: "sm",
    showUnreadCount: true,
  },
};

export const Large: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    size: "lg",
    showUnreadCount: true,
    groupByDate: true,
    showTimestamps: true,
    showCategories: true,
    showPriorities: true,
  },
};

// Positions
export const TopLeft: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    position: "top-left",
    showUnreadCount: true,
  },
};

export const BottomRight: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    position: "bottom-right",
    showUnreadCount: true,
  },
};

export const BottomLeft: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    position: "bottom-left",
    showUnreadCount: true,
  },
};

// Filtering
export const UnreadOnly: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    statusFilter: "unread",
    showUnreadCount: true,
  },
};

export const SystemNotifications: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    categoryFilter: "system",
    showUnreadCount: true,
  },
};

export const HighPriority: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    priorityFilter: "high",
    showUnreadCount: true,
  },
};

// Grouping and display options
export const NoGrouping: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    groupByDate: false,
    showUnreadCount: true,
  },
};

export const WithCategories: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    showCategories: true,
    showUnreadCount: true,
  },
};

export const WithPriorities: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    showPriorities: true,
    showUnreadCount: true,
  },
};

export const WithAllMetadata: Story = {
  args: {
    notifications: sampleNotifications,
    open: true,
    showCategories: true,
    showPriorities: true,
    showTimestamps: true,
    groupByDate: true,
    showUnreadCount: true,
  },
};

// Empty states
export const Empty: Story = {
  args: {
    notifications: [],
    open: true,
    showUnreadCount: true,
  },
};

export const EmptyWithCustomMessage: Story = {
  args: {
    notifications: [],
    open: true,
    emptyMessage: "No notifications yet",
    showUnreadCount: true,
  },
};

// Different notification types
export const SuccessNotifications: Story = {
  args: {
    notifications: [
      {
        id: "1",
        title: "Account Created",
        description: "Your account has been successfully created.",
        variant: "success",
        priority: "medium",
        status: "unread",
        category: "user",
        timestamp: new Date(Date.now() - 5 * 60 * 1000),
      },
      {
        id: "2",
        title: "Payment Successful",
        description: "Your payment has been processed successfully.",
        variant: "success",
        priority: "medium",
        status: "read",
        category: "system",
        timestamp: new Date(Date.now() - 2 * 60 * 60 * 1000),
      },
    ],
    open: true,
    showUnreadCount: true,
  },
};

export const ErrorNotifications: Story = {
  args: {
    notifications: [
      {
        id: "1",
        title: "Connection Failed",
        description:
          "Unable to connect to the server. Please check your internet connection.",
        variant: "error",
        priority: "high",
        status: "unread",
        category: "system",
        timestamp: new Date(Date.now() - 10 * 60 * 1000),
        actions: [
          {
            label: "Retry",
            onClick: () => alert("Retrying..."),
            variant: "primary" as const,
          },
          {
            label: "Check Status",
            onClick: () => alert("Checking status..."),
            variant: "secondary" as const,
          },
        ],
      },
      {
        id: "2",
        title: "Payment Failed",
        description:
          "Your payment could not be processed. Please update your payment method.",
        variant: "error",
        priority: "urgent",
        status: "unread",
        category: "system",
        timestamp: new Date(Date.now() - 30 * 60 * 1000),
        actions: [
          {
            label: "Update Payment",
            onClick: () => alert("Updating payment..."),
            variant: "primary" as const,
          },
        ],
      },
    ],
    open: true,
    showUnreadCount: true,
  },
};

export const WarningNotifications: Story = {
  args: {
    notifications: [
      {
        id: "1",
        title: "Storage Almost Full",
        description:
          "You have used 90% of your storage quota. Consider upgrading your plan.",
        variant: "warning",
        priority: "medium",
        status: "unread",
        category: "system",
        timestamp: new Date(Date.now() - 15 * 60 * 1000),
        actions: [
          {
            label: "Upgrade Plan",
            onClick: () => alert("Upgrading plan..."),
            variant: "primary" as const,
          },
          {
            label: "Free Up Space",
            onClick: () => alert("Freeing up space..."),
            variant: "secondary" as const,
          },
        ],
      },
      {
        id: "2",
        title: "Security Alert",
        description: "A new device has logged into your account.",
        variant: "warning",
        priority: "high",
        status: "read",
        category: "security",
        timestamp: new Date(Date.now() - 1 * 60 * 60 * 1000),
        actions: [
          {
            label: "Review Activity",
            onClick: () => alert("Reviewing activity..."),
            variant: "primary" as const,
          },
        ],
      },
    ],
    open: true,
    showUnreadCount: true,
  },
};

export const AnnouncementNotifications: Story = {
  args: {
    notifications: [
      {
        id: "1",
        title: "New Dashboard Available",
        description:
          "Check out our redesigned dashboard with enhanced analytics and improved performance.",
        variant: "announcement",
        priority: "medium",
        status: "unread",
        category: "feature",
        timestamp: new Date(Date.now() - 20 * 60 * 1000),
        actions: [
          {
            label: "Explore Dashboard",
            onClick: () => alert("Exploring dashboard..."),
            variant: "primary" as const,
          },
          {
            label: "View Changelog",
            onClick: () => alert("Viewing changelog..."),
            variant: "secondary" as const,
          },
        ],
      },
      {
        id: "2",
        title: "API v2 Released",
        description:
          "Our new API version is now available with improved performance and new endpoints.",
        variant: "announcement",
        priority: "medium",
        status: "read",
        category: "feature",
        timestamp: new Date(Date.now() - 3 * 60 * 60 * 1000),
        actions: [
          {
            label: "View Documentation",
            onClick: () => alert("Viewing docs..."),
            variant: "primary" as const,
          },
        ],
      },
    ],
    open: true,
    showUnreadCount: true,
  },
};

export const PromotionNotifications: Story = {
  args: {
    notifications: [
      {
        id: "1",
        title: "Black Friday Sale",
        description: "Get 50% off on all premium features. Limited time offer!",
        variant: "promotion",
        priority: "low",
        status: "unread",
        category: "marketing",
        timestamp: new Date(Date.now() - 25 * 60 * 1000),
        actions: [
          {
            label: "Claim Offer",
            onClick: () => alert("Claiming offer..."),
            variant: "primary" as const,
          },
          {
            label: "Learn More",
            onClick: () => alert("Learning more..."),
            variant: "secondary" as const,
          },
        ],
      },
      {
        id: "2",
        title: "Referral Bonus",
        description:
          "Invite friends and earn credits for each successful referral.",
        variant: "promotion",
        priority: "low",
        status: "read",
        category: "marketing",
        timestamp: new Date(Date.now() - 4 * 60 * 60 * 1000),
        actions: [
          {
            label: "Invite Friends",
            onClick: () => alert("Inviting friends..."),
            variant: "primary" as const,
          },
        ],
      },
    ],
    open: true,
    showUnreadCount: true,
  },
};

// Interactive example
export const Interactive: Story = {
  render: () => <InteractiveExample />,
};

// All variants comparison
export const AllVariants: Story = {
  args: {
    notifications: [
      {
        id: "1",
        title: "Info Notification",
        description: "This is an informational notification.",
        variant: "info",
        priority: "medium",
        status: "unread",
        category: "system",
        timestamp: new Date(Date.now() - 1 * 60 * 1000),
      },
      {
        id: "2",
        title: "Success Notification",
        description: "This is a success notification.",
        variant: "success",
        priority: "medium",
        status: "unread",
        category: "system",
        timestamp: new Date(Date.now() - 2 * 60 * 1000),
      },
      {
        id: "3",
        title: "Warning Notification",
        description: "This is a warning notification.",
        variant: "warning",
        priority: "high",
        status: "unread",
        category: "system",
        timestamp: new Date(Date.now() - 3 * 60 * 1000),
      },
      {
        id: "4",
        title: "Error Notification",
        description: "This is an error notification.",
        variant: "error",
        priority: "urgent",
        status: "unread",
        category: "system",
        timestamp: new Date(Date.now() - 4 * 60 * 1000),
      },
      {
        id: "5",
        title: "Announcement Notification",
        description: "This is an announcement notification.",
        variant: "announcement",
        priority: "medium",
        status: "unread",
        category: "feature",
        timestamp: new Date(Date.now() - 5 * 60 * 1000),
      },
      {
        id: "6",
        title: "Promotion Notification",
        description: "This is a promotion notification.",
        variant: "promotion",
        priority: "low",
        status: "unread",
        category: "marketing",
        timestamp: new Date(Date.now() - 6 * 60 * 1000),
      },
    ],
    open: true,
    showUnreadCount: true,
    showCategories: true,
    showPriorities: true,
  },
};

// Complex example with many notifications
export const ManyNotifications: Story = {
  args: {
    notifications: Array.from({ length: 20 }, (_, i) => ({
      id: `notification-${i + 1}`,
      title: `Notification ${i + 1}`,
      description: `This is notification number ${i + 1} with some description text.`,
      variant: [
        "info",
        "success",
        "warning",
        "error",
        "announcement",
        "promotion",
      ][i % 6] as any,
      priority: ["low", "medium", "high", "urgent"][i % 4] as any,
      status: ["unread", "read", "archived"][i % 3] as any,
      category: [
        "system",
        "user",
        "security",
        "feature",
        "marketing",
        "maintenance",
      ][i % 6] as any,
      timestamp: new Date(Date.now() - i * 60 * 60 * 1000), // i hours ago
      actions:
        i % 3 === 0
          ? [
              {
                label: "Action 1",
                onClick: () => alert(`Action 1 for notification ${i + 1}`),
                variant: "primary" as const,
              },
              {
                label: "Action 2",
                onClick: () => alert(`Action 2 for notification ${i + 1}`),
                variant: "secondary" as const,
              },
            ]
          : undefined,
    })),
    open: true,
    showUnreadCount: true,
    groupByDate: true,
    showTimestamps: true,
    showCategories: true,
    showPriorities: true,
  },
};
