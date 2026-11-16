import type { Meta, StoryObj } from "@storybook/react-vite";
import { Progress, CircularProgress, Text } from "@calimero-network/mero-ui";

const meta: Meta = {
  title: "Feedback/Progress",
  parameters: {
    layout: "padded",
  },
};

export default meta;

export const LinearProgress: StoryObj = {
  render: () => (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: "24px",
        width: "400px",
      }}
    >
      <div>
        <Text size="sm" style={{ marginBottom: "8px" }}>
          Default Progress
        </Text>
        <Progress value={30} />
      </div>

      <div>
        <Text size="sm" style={{ marginBottom: "8px" }}>
          With Label and Value
        </Text>
        <Progress value={60} label="Loading..." showValue />
      </div>

      <div>
        <Text size="sm" style={{ marginBottom: "8px" }}>
          Success Variant
        </Text>
        <Progress value={100} variant="success" showValue />
      </div>
    </div>
  ),
};

export const Sizes: StoryObj = {
  render: () => (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: "16px",
        width: "400px",
      }}
    >
      <div>
        <Text size="sm" style={{ marginBottom: "8px" }}>
          Small
        </Text>
        <Progress value={40} size="sm" />
      </div>

      <div>
        <Text size="sm" style={{ marginBottom: "8px" }}>
          Medium
        </Text>
        <Progress value={60} size="md" />
      </div>

      <div>
        <Text size="sm" style={{ marginBottom: "8px" }}>
          Large
        </Text>
        <Progress value={80} size="lg" />
      </div>
    </div>
  ),
};

export const Variants: StoryObj = {
  render: () => (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: "16px",
        width: "400px",
      }}
    >
      <div>
        <Text size="sm" style={{ marginBottom: "8px" }}>
          Default
        </Text>
        <Progress value={30} variant="default" />
      </div>

      <div>
        <Text size="sm" style={{ marginBottom: "8px" }}>
          Success
        </Text>
        <Progress value={70} variant="success" />
      </div>

      <div>
        <Text size="sm" style={{ marginBottom: "8px" }}>
          Warning
        </Text>
        <Progress value={50} variant="warning" />
      </div>

      <div>
        <Text size="sm" style={{ marginBottom: "8px" }}>
          Error
        </Text>
        <Progress value={20} variant="error" />
      </div>
    </div>
  ),
};

export const CircularProgressStory: StoryObj = {
  render: () => (
    <div style={{ display: "flex", gap: "32px", alignItems: "center" }}>
      <CircularProgress value={25} size="sm" showValue />
      <CircularProgress value={50} size="md" showValue />
      <CircularProgress value={75} size="lg" showValue />
      <CircularProgress value={100} size="xl" showValue />
    </div>
  ),
};

export const CircularVariants: StoryObj = {
  render: () => (
    <div style={{ display: "flex", gap: "32px", alignItems: "center" }}>
      <CircularProgress value={60} variant="default" showValue />
      <CircularProgress value={80} variant="success" showValue />
      <CircularProgress value={40} variant="warning" showValue />
      <CircularProgress value={20} variant="error" showValue />
    </div>
  ),
};

export const WithLabels: StoryObj = {
  render: () => (
    <div style={{ display: "flex", gap: "32px", alignItems: "center" }}>
      <CircularProgress value={75} label="Upload Progress" showValue />
      <CircularProgress
        value={90}
        label="Download Complete"
        variant="success"
        showValue
      />
    </div>
  ),
};
