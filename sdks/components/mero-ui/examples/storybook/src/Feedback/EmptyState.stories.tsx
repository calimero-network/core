import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  EmptyState,
  EmptySearchState,
  EmptyDataState,
  ErrorView,
  EmptyView,
  NotFoundView,
} from "@calimero-network/mero-ui";
import { Icon } from "@calimero-network/mero-ui";
import { Button } from "@calimero-network/mero-ui";

const meta: Meta<typeof EmptyState> = {
  title: "Feedback/Empty State",
  component: EmptyState,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    title: {
      control: { type: "text" },
      description: "Empty state title",
    },
    description: {
      control: { type: "text" },
      description: "Empty state description",
    },
    size: {
      control: { type: "select" },
      options: ["sm", "md", "lg"],
      description: "Size variant",
    },
    variant: {
      control: { type: "select" },
      options: ["default", "minimal", "card"],
      description: "Visual variant",
    },
    type: {
      control: { type: "select" },
      options: ["empty", "error", "not-found", "loading"],
      description: "State type",
    },
  },
};

export default meta;
type Story = StoryObj<typeof EmptyState>;

export const Default: Story = {
  args: {
    icon: <Icon name="inbox" size="lg" />,
    title: "No items found",
    description: "There are no items to display at the moment.",
    size: "md",
    variant: "default",
  },
};

export const WithAction: Story = {
  args: {
    icon: <Icon name="plus" size="lg" />,
    title: "Create your first item",
    description: "Get started by creating your first item to see it here.",
    action: <Button variant="primary">Create Item</Button>,
    size: "md",
    variant: "default",
  },
};

export const CardVariant: Story = {
  args: {
    icon: <Icon name="folder" size="lg" />,
    title: "No folders",
    description: "You haven't created any folders yet.",
    size: "md",
    variant: "card",
  },
};

export const MinimalVariant: Story = {
  args: {
    icon: <Icon name="search" size="md" />,
    title: "No results",
    description: "Try adjusting your search terms.",
    size: "sm",
    variant: "minimal",
  },
};

export const AllSizes: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "24px" }}>
      <EmptyState
        icon={<Icon name="inbox" size="sm" />}
        title="Small empty state"
        description="This is a small empty state."
        size="sm"
      />
      <EmptyState
        icon={<Icon name="inbox" size="md" />}
        title="Medium empty state"
        description="This is a medium empty state."
        size="md"
      />
      <EmptyState
        icon={<Icon name="inbox" size="lg" />}
        title="Large empty state"
        description="This is a large empty state."
        size="lg"
      />
    </div>
  ),
};

export const SearchEmpty: Story = {
  render: () => (
    <EmptySearchState
      searchTerm="react components"
      onClear={() => console.log("Clear search")}
    />
  ),
};

export const DataEmpty: Story = {
  render: () => (
    <EmptyDataState onRefresh={() => console.log("Refresh data")} />
  ),
};

export const ErrorState: Story = {
  args: {
    title: "Something went wrong",
    description: "An unexpected error occurred. Please try again.",
    type: "error",
    action: <Button variant="primary">Try Again</Button>,
  },
};

export const NotFoundState: Story = {
  args: {
    title: "Page not found",
    description: "The page you are looking for does not exist.",
    type: "not-found",
    action: <Button variant="primary">Go Home</Button>,
  },
};

export const LoadingState: Story = {
  args: {
    title: "Loading...",
    description: "Please wait while we fetch your data.",
    type: "loading",
  },
};

export const AllTypes: Story = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "24px" }}>
      <EmptyState
        title="No items"
        description="This list is empty"
        type="empty"
      />
      <EmptyState
        title="Error occurred"
        description="Something went wrong"
        type="error"
      />
      <EmptyState
        title="Not found"
        description="Page does not exist"
        type="not-found"
      />
      <EmptyState title="Loading..." description="Please wait" type="loading" />
    </div>
  ),
};

// Legacy ErrorView stories
export const LegacyError: Story = {
  render: () => (
    <ErrorView
      title="Something went wrong"
      message="An unexpected error occurred. Please try again."
      actionLabel="Try Again"
      onAction={() => console.log("Try again")}
    />
  ),
  component: ErrorView,
};

export const LegacyEmpty: Story = {
  render: () => (
    <EmptyView title="No data" message="There's no data to display." />
  ),
  component: EmptyView,
};

export const LegacyNotFound: Story = {
  render: () => (
    <NotFoundView
      title="Page not found"
      message="The page you are looking for does not exist."
      actionLabel="Go Home"
      onAction={() => console.log("Go home")}
    />
  ),
  component: NotFoundView,
};
