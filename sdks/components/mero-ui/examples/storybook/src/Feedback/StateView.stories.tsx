import type { Meta, StoryObj } from "@storybook/react-vite";
import { ErrorView, EmptyView, NotFoundView } from "@calimero-network/mero-ui";

const meta: Meta<typeof ErrorView> = {
  title: "Feedback/State View",
  component: ErrorView,
  parameters: {
    layout: "centered",
  },
  argTypes: {
    title: {
      control: { type: "text" },
      description: "State view title",
    },
    message: {
      control: { type: "text" },
      description: "State view message",
    },
    actionLabel: {
      control: { type: "text" },
      description: "Action button label",
    },
    variant: {
      control: { type: "select" },
      options: ["error", "empty", "not-found"],
      description: "State variant (error, empty, or not-found)",
    },
    showAction: {
      control: { type: "boolean" },
      description: "Whether to show the action button",
    },
  },
};

export default meta;
type Story = StoryObj<typeof ErrorView>;

export const Error: Story = {
  args: {
    title: "Something went wrong",
    message: "An unexpected error occurred. Please try again.",
    actionLabel: "Try Again",
    variant: "error",
    showAction: true,
    onAction: () => alert("Retry action clicked"),
  },
};

export const Empty: Story = {
  render: (args) => (
    <EmptyView {...args} onAction={() => alert("Get started action clicked")} />
  ),
  args: {
    title: "No data available",
    message: "There are no items to display at the moment.",
    actionLabel: "Get Started",
  },
};

export const NotFound: Story = {
  render: (args) => (
    <NotFoundView {...args} onAction={() => alert("Go back action clicked")} />
  ),
  args: {
    title: "Page not found",
    message: "The page you are looking for does not exist.",
    actionLabel: "Go Back",
  },
};

export const CustomIcon: Story = {
  args: {
    title: "Custom Error",
    message: "This error view has a custom icon.",
    actionLabel: "Retry",
    variant: "error",
    showAction: true,
    onAction: () => alert("Custom retry action clicked"),
    icon: (
      <div
        style={{
          width: "64px",
          height: "64px",
          borderRadius: "50%",
          backgroundColor: "var(--color-background-secondary)",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          marginBottom: "16px",
          color: "var(--color-semantic-warning)",
        }}
      >
        <svg
          width="32"
          height="32"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
        >
          <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z" />
          <line x1="12" y1="9" x2="12" y2="13" />
          <line x1="12" y1="17" x2="12.01" y2="17" />
        </svg>
      </div>
    ),
  },
};

export const NoAction: Story = {
  args: {
    title: "Error without action",
    message: "This error view does not have an action button.",
    variant: "error",
    showAction: false,
  },
};

export const LongMessage: Story = {
  args: {
    title: "Detailed Error Information",
    message:
      "This is a longer error message that demonstrates how the component handles text wrapping and longer content. The message should wrap properly and maintain good readability while staying within the maximum width constraints.",
    actionLabel: "Contact Support",
    variant: "error",
    showAction: true,
    onAction: () => alert("Contact support action clicked"),
  },
};
