import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  Banner,
  InfoBanner,
  SuccessBanner,
  WarningBanner,
  ErrorBanner,
  AnnouncementBanner,
  PromotionBanner,
} from "@calimero-network/mero-ui";
import { useState } from "react";

const meta: Meta<typeof Banner> = {
  title: "Feedback/Banner",
  component: Banner,
  parameters: {
    layout: "padded",
    docs: {
      description: {
        component:
          "A flexible banner component for announcements, notifications, and promotional content. Supports multiple variants, sizes, positions, and interactive features.",
      },
    },
  },
  argTypes: {
    variant: {
      control: "select",
      options: [
        "info",
        "success",
        "warning",
        "error",
        "announcement",
        "promotion",
      ],
      description: "Visual variant of the banner",
    },
    size: {
      control: "select",
      options: ["sm", "md", "lg"],
      description: "Size of the banner",
    },
    position: {
      control: "select",
      options: ["top", "bottom", "inline"],
      description: "Position of the banner",
    },
    title: {
      control: "text",
      description: "Banner title",
    },
    description: {
      control: "text",
      description: "Banner description",
    },
    dismissible: {
      control: "boolean",
      description: "Whether the banner can be dismissed",
    },
    sticky: {
      control: "boolean",
      description: "Whether the banner sticks to its position",
    },
    animated: {
      control: "boolean",
      description: "Whether to show animations",
    },
    autoDismiss: {
      control: "number",
      description: "Auto dismiss after delay (milliseconds)",
    },
  },
};

export default meta;
type Story = StoryObj<typeof Banner>;

// Basic examples
export const Default: Story = {
  args: {
    title: "Welcome to Calimero",
    description:
      "This is a default info banner with some important information.",
    variant: "info",
    size: "md",
    position: "inline",
  },
};

export const WithTitleOnly: Story = {
  args: {
    title: "System Maintenance",
    variant: "warning",
    size: "md",
  },
};

export const WithDescriptionOnly: Story = {
  args: {
    description:
      "Your account has been successfully updated with the latest settings.",
    variant: "success",
    size: "md",
  },
};

// Variants
export const Info: Story = {
  args: {
    title: "Information",
    description: "This is an informational banner with useful details.",
    variant: "info",
  },
};

export const Success: Story = {
  args: {
    title: "Success!",
    description: "Your action was completed successfully.",
    variant: "success",
  },
};

export const Warning: Story = {
  args: {
    title: "Warning",
    description: "Please review your settings before proceeding.",
    variant: "warning",
  },
};

export const Error: Story = {
  args: {
    title: "Error",
    description: "Something went wrong. Please try again.",
    variant: "error",
  },
};

export const Announcement: Story = {
  args: {
    title: "New Feature Available",
    description:
      "Check out our latest dashboard improvements and enhanced analytics.",
    variant: "announcement",
  },
};

export const Promotion: Story = {
  args: {
    title: "Special Offer",
    description: "Get 50% off on all premium features this month only!",
    variant: "promotion",
  },
};

// Sizes
export const Small: Story = {
  args: {
    title: "Small Banner",
    description: "This is a small-sized banner.",
    size: "sm",
  },
};

export const Medium: Story = {
  args: {
    title: "Medium Banner",
    description: "This is a medium-sized banner (default).",
    size: "md",
  },
};

export const Large: Story = {
  args: {
    title: "Large Banner",
    description:
      "This is a large-sized banner for more prominent announcements.",
    size: "lg",
  },
};

// With actions
export const WithActions: Story = {
  args: {
    title: "Update Available",
    description: "A new version of the application is ready to install.",
    variant: "info",
    actions: [
      {
        label: "Update Now",
        onClick: () => alert("Updating..."),
        variant: "primary" as const,
      },
      {
        label: "Later",
        onClick: () => alert("Reminded later"),
        variant: "secondary" as const,
      },
    ],
  },
};

export const WithSingleAction: Story = {
  args: {
    title: "Terms Updated",
    description:
      "Our terms of service have been updated. Please review the changes.",
    variant: "warning",
    actions: [
      {
        label: "Review Terms",
        onClick: () => alert("Opening terms..."),
        variant: "primary" as const,
      },
    ],
  },
};

export const WithDisabledAction: Story = {
  args: {
    title: "Feature Coming Soon",
    description: "This feature is currently under development.",
    variant: "announcement",
    actions: [
      {
        label: "Notify Me",
        onClick: () => alert("Notification set!"),
        variant: "primary" as const,
        disabled: true,
      },
    ],
  },
};

// Dismissible
export const Dismissible: Story = {
  args: {
    title: "Dismissible Banner",
    description: "This banner can be closed by clicking the X button.",
    variant: "info",
    dismissible: true,
    onClose: () => alert("Banner closed!"),
  },
};

export const AutoDismiss: Story = {
  args: {
    title: "Auto Dismiss",
    description: "This banner will automatically disappear in 3 seconds.",
    variant: "success",
    autoDismiss: 3000,
    onClose: () => alert("Banner auto-dismissed!"),
  },
};

// Positions
export const TopPosition: Story = {
  args: {
    title: "Top Banner",
    description: "This banner is positioned at the top of the viewport.",
    variant: "info",
    position: "top",
    dismissible: true,
  },
  parameters: {
    layout: "fullscreen",
  },
};

export const BottomPosition: Story = {
  args: {
    title: "Bottom Banner",
    description: "This banner is positioned at the bottom of the viewport.",
    variant: "warning",
    position: "bottom",
    dismissible: true,
  },
  parameters: {
    layout: "fullscreen",
  },
};

export const InlinePosition: Story = {
  args: {
    title: "Inline Banner",
    description: "This banner is positioned inline with the content (default).",
    variant: "success",
    position: "inline",
  },
};

// Sticky
export const Sticky: Story = {
  args: {
    title: "Sticky Banner",
    description: "This banner sticks to the top when scrolling.",
    variant: "announcement",
    sticky: true,
    dismissible: true,
  },
  render: (args) => (
    <div style={{ height: "200vh", padding: "20px" }}>
      <Banner {...args} />
      <div style={{ marginTop: "20px" }}>
        <h3>Scroll down to see the sticky behavior</h3>
        <p>
          This content is here to demonstrate the sticky banner behavior. The
          banner should remain at the top when you scroll down.
        </p>
        <div
          style={{
            height: "100vh",
            background: "var(--color-neutral-800)",
            margin: "20px 0",
            borderRadius: "8px",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: "var(--color-neutral-300)",
          }}
        >
          Scroll content area
        </div>
      </div>
    </div>
  ),
};

// Custom styling
export const CustomColors: Story = {
  args: {
    title: "Custom Styled Banner",
    description:
      "This banner uses custom colors for background, text, and border.",
    backgroundColor: "linear-gradient(135deg, #667eea 0%, #764ba2 100%)",
    textColor: "#FFFFFF",
    borderColor: "#5a67d8",
    dismissible: true,
  },
};

export const CustomIcon: Story = {
  args: {
    title: "Custom Icon",
    description: "This banner uses a custom icon instead of the default one.",
    icon: (
      <svg
        width="20"
        height="20"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
      >
        <path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z" />
      </svg>
    ),
    variant: "announcement",
  },
};

// Animation
export const WithoutAnimation: Story = {
  args: {
    title: "No Animation",
    description: "This banner appears and disappears without animation.",
    variant: "info",
    animated: false,
    dismissible: true,
  },
};

// Interactive examples
export const InteractiveExample: Story = {
  render: () => {
    const [visible, setVisible] = useState(true);
    const [count, setCount] = useState(0);

    return (
      <div>
        {visible && (
          <Banner
            title="Interactive Banner"
            description={`This banner has been interacted with ${count} times.`}
            variant="info"
            dismissible
            onClose={() => setVisible(false)}
            actions={[
              {
                label: "Increment",
                onClick: () => setCount(count + 1),
                variant: "primary" as const,
              },
              {
                label: "Reset",
                onClick: () => setCount(0),
                variant: "secondary" as const,
              },
            ]}
          />
        )}
        {!visible && (
          <button
            onClick={() => {
              setVisible(true);
              setCount(0);
            }}
            style={{
              padding: "8px 16px",
              background: "var(--color-brand-600)",
              color: "white",
              border: "none",
              borderRadius: "4px",
              cursor: "pointer",
            }}
          >
            Show Banner Again
          </button>
        )}
      </div>
    );
  },
};

// Convenience components
export const ConvenienceComponents: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <InfoBanner
        title="Info Banner"
        description="Using the InfoBanner convenience component."
      />
      <SuccessBanner
        title="Success Banner"
        description="Using the SuccessBanner convenience component."
      />
      <WarningBanner
        title="Warning Banner"
        description="Using the WarningBanner convenience component."
      />
      <ErrorBanner
        title="Error Banner"
        description="Using the ErrorBanner convenience component."
      />
      <AnnouncementBanner
        title="Announcement Banner"
        description="Using the AnnouncementBanner convenience component."
      />
      <PromotionBanner
        title="Promotion Banner"
        description="Using the PromotionBanner convenience component."
      />
    </div>
  ),
};

// All variants comparison
export const AllVariants: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <Banner
        title="Info Banner"
        description="Information message"
        variant="info"
      />
      <Banner
        title="Success Banner"
        description="Success message"
        variant="success"
      />
      <Banner
        title="Warning Banner"
        description="Warning message"
        variant="warning"
      />
      <Banner
        title="Error Banner"
        description="Error message"
        variant="error"
      />
      <Banner
        title="Announcement Banner"
        description="Announcement message"
        variant="announcement"
      />
      <Banner
        title="Promotion Banner"
        description="Promotion message"
        variant="promotion"
      />
    </div>
  ),
};

// All sizes comparison
export const AllSizes: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <Banner title="Small Banner" description="Small size banner" size="sm" />
      <Banner
        title="Medium Banner"
        description="Medium size banner (default)"
        size="md"
      />
      <Banner title="Large Banner" description="Large size banner" size="lg" />
    </div>
  ),
};

// Complex example
export const ComplexExample: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "20px" }}>
      <Banner
        title="🎉 New Dashboard Features Available"
        description="We've added advanced analytics, custom widgets, and improved performance monitoring. Check out what's new!"
        variant="announcement"
        size="lg"
        actions={[
          {
            label: "Explore Features",
            onClick: () => alert("Opening feature tour..."),
            variant: "primary" as const,
          },
          {
            label: "View Changelog",
            onClick: () => alert("Opening changelog..."),
            variant: "secondary" as const,
          },
        ]}
        dismissible
        onClose={() => alert("Banner dismissed")}
      />

      <Banner
        title="⚠️ Maintenance Window"
        description="Scheduled maintenance will occur on Sunday, 2:00 AM - 4:00 AM UTC. Some features may be temporarily unavailable."
        variant="warning"
        actions={[
          {
            label: "Learn More",
            onClick: () => alert("Opening maintenance info..."),
            variant: "primary" as const,
          },
        ]}
        dismissible
      />

      <Banner
        title="💡 Pro Tip"
        description="Use keyboard shortcuts to navigate faster. Press Ctrl+K to open the command palette."
        variant="info"
        size="sm"
        dismissible
      />
    </div>
  ),
};
