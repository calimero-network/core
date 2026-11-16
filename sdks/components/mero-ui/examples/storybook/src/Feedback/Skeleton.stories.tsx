import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  Skeleton,
  SkeletonText,
  SkeletonTitle,
  SkeletonAvatar,
  SkeletonButton,
  SkeletonCard,
  SkeletonTable,
} from "@calimero-network/mero-ui";

const meta: Meta<typeof Skeleton> = {
  title: "Feedback/Skeleton",
  component: Skeleton,
  parameters: {
    layout: "padded",
  },
  argTypes: {
    width: {
      control: { type: "text" },
      description: "Skeleton width",
    },
    height: {
      control: { type: "text" },
      description: "Skeleton height",
    },
    variant: {
      control: { type: "select" },
      options: ["text", "rectangular", "circular", "rounded"],
      description: "Skeleton variant",
    },
    animation: {
      control: { type: "select" },
      options: ["pulse", "wave", "none"],
      description: "Animation type",
    },
    lines: {
      control: { type: "number" },
      description: "Number of lines for text variant",
    },
  },
};

export default meta;
type Story = StoryObj<typeof Skeleton>;

export const Default: Story = {
  args: {
    width: "100%",
    height: "20px",
  },
};

export const Text: Story = {
  args: {
    variant: "text",
    width: "100%",
    height: "16px",
    lines: 3,
  },
};

export const Rectangular: Story = {
  args: {
    variant: "rectangular",
    width: "200px",
    height: "100px",
  },
};

export const Circular: Story = {
  args: {
    variant: "circular",
    width: "50px",
    height: "50px",
  },
};

export const Rounded: Story = {
  args: {
    variant: "rounded",
    width: "150px",
    height: "40px",
  },
};

export const PulseAnimation: Story = {
  args: {
    width: "100%",
    height: "20px",
    animation: "pulse",
  },
};

export const WaveAnimation: Story = {
  args: {
    width: "100%",
    height: "20px",
    animation: "wave",
  },
};

export const NoAnimation: Story = {
  args: {
    width: "100%",
    height: "20px",
    animation: "none",
  },
};

export const MultipleLines: Story = {
  args: {
    variant: "text",
    width: "100%",
    height: "16px",
    lines: 5,
  },
};

export const SkeletonTextStory: Story = {
  render: () => (
    <div>
      <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>Skeleton Text</h3>
      <SkeletonText lines={3} />
    </div>
  ),
};

export const SkeletonTitleStory: Story = {
  render: () => (
    <div>
      <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>Skeleton Title</h3>
      <SkeletonTitle />
    </div>
  ),
};

export const SkeletonAvatarStory: Story = {
  render: () => (
    <div>
      <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
        Skeleton Avatar
      </h3>
      <div style={{ display: "flex", gap: "16px", alignItems: "center" }}>
        <SkeletonAvatar size={40} />
        <SkeletonAvatar size={50} />
        <SkeletonAvatar size={60} />
      </div>
    </div>
  ),
};

export const SkeletonButtonStory: Story = {
  render: () => (
    <div>
      <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>
        Skeleton Button
      </h3>
      <div style={{ display: "flex", gap: "12px" }}>
        <SkeletonButton width="80px" />
        <SkeletonButton width="120px" />
        <SkeletonButton width="100px" />
      </div>
    </div>
  ),
};

export const SkeletonCardStory: Story = {
  render: () => (
    <div>
      <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>Skeleton Card</h3>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))",
          gap: "16px",
        }}
      >
        <SkeletonCard />
        <SkeletonCard />
        <SkeletonCard />
      </div>
    </div>
  ),
};

export const SkeletonTableStory: Story = {
  render: () => (
    <div>
      <h3 style={{ color: "#FFFFFF", marginBottom: "16px" }}>Skeleton Table</h3>
      <SkeletonTable rows={5} columns={4} />
    </div>
  ),
};

export const LoadingStates: Story = {
  render: () => (
    <div>
      <h3 style={{ color: "#FFFFFF", marginBottom: "20px" }}>Loading States</h3>

      <div style={{ marginBottom: "32px" }}>
        <h4 style={{ color: "#FFFFFF", marginBottom: "12px" }}>
          Article Loading
        </h4>
        <div style={{ maxWidth: "600px" }}>
          <SkeletonTitle width="70%" style={{ marginBottom: "12px" }} />
          <SkeletonText lines={4} style={{ marginBottom: "16px" }} />
          <div style={{ display: "flex", gap: "12px", alignItems: "center" }}>
            <SkeletonAvatar size={32} />
            <Skeleton width="120px" height="16px" />
            <Skeleton width="80px" height="16px" />
          </div>
        </div>
      </div>

      <div style={{ marginBottom: "32px" }}>
        <h4 style={{ color: "#FFFFFF", marginBottom: "12px" }}>
          Profile Loading
        </h4>
        <div style={{ display: "flex", gap: "16px", alignItems: "flex-start" }}>
          <SkeletonAvatar size={80} />
          <div style={{ flex: 1 }}>
            <SkeletonTitle width="200px" style={{ marginBottom: "8px" }} />
            <SkeletonText lines={2} style={{ marginBottom: "12px" }} />
            <div style={{ display: "flex", gap: "8px" }}>
              <SkeletonButton width="80px" />
              <SkeletonButton width="100px" />
            </div>
          </div>
        </div>
      </div>

      <div>
        <h4 style={{ color: "#FFFFFF", marginBottom: "12px" }}>
          Dashboard Loading
        </h4>
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fit, minmax(250px, 1fr))",
            gap: "16px",
          }}
        >
          <SkeletonCard />
          <SkeletonCard />
          <SkeletonCard />
          <SkeletonCard />
        </div>
      </div>
    </div>
  ),
};

export const CustomSizes: Story = {
  render: () => (
    <div>
      <h3 style={{ color: "#FFFFFF", marginBottom: "20px" }}>Custom Sizes</h3>

      <div style={{ marginBottom: "20px" }}>
        <h4 style={{ color: "#FFFFFF", marginBottom: "8px" }}>
          Different Widths
        </h4>
        <div style={{ display: "flex", flexDirection: "column", gap: "8px" }}>
          <Skeleton width="25%" height="16px" />
          <Skeleton width="50%" height="16px" />
          <Skeleton width="75%" height="16px" />
          <Skeleton width="100%" height="16px" />
        </div>
      </div>

      <div style={{ marginBottom: "20px" }}>
        <h4 style={{ color: "#FFFFFF", marginBottom: "8px" }}>
          Different Heights
        </h4>
        <div style={{ display: "flex", alignItems: "end", gap: "8px" }}>
          <Skeleton width="60px" height="20px" />
          <Skeleton width="60px" height="30px" />
          <Skeleton width="60px" height="40px" />
          <Skeleton width="60px" height="50px" />
        </div>
      </div>

      <div>
        <h4 style={{ color: "#FFFFFF", marginBottom: "8px" }}>
          Mixed Variants
        </h4>
        <div style={{ display: "flex", alignItems: "center", gap: "16px" }}>
          <Skeleton variant="circular" width="40px" height="40px" />
          <div style={{ flex: 1 }}>
            <Skeleton
              width="60%"
              height="16px"
              style={{ marginBottom: "4px" }}
            />
            <Skeleton width="40%" height="14px" />
          </div>
          <Skeleton variant="rounded" width="80px" height="32px" />
        </div>
      </div>
    </div>
  ),
};
