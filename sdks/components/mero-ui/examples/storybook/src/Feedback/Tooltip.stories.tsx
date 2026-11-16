import type { Meta, StoryObj } from "@storybook/react-vite";
import { Tooltip, Button, Text } from "@calimero-network/mero-ui";

const meta: Meta = {
  title: "Feedback/Tooltip",
  parameters: {
    layout: "padded",
  },
};

export default meta;

export const Basic: StoryObj = {
  render: () => (
    <div
      style={{
        display: "flex",
        gap: "32px",
        alignItems: "center",
        padding: "40px",
      }}
    >
      <Tooltip content="This is a tooltip">
        <Button>Hover me</Button>
      </Tooltip>

      <Tooltip content="Click to show tooltip" trigger="click">
        <Button>Click me</Button>
      </Tooltip>

      <Tooltip content="Focus to show tooltip" trigger="focus">
        <Button>Focus me</Button>
      </Tooltip>
    </div>
  ),
};

export const Placements: StoryObj = {
  render: () => (
    <div
      style={{
        display: "flex",
        gap: "32px",
        alignItems: "center",
        padding: "40px",
      }}
    >
      <Tooltip content="Top tooltip" placement="top">
        <Button>Top</Button>
      </Tooltip>

      <Tooltip content="Right tooltip" placement="right">
        <Button>Right</Button>
      </Tooltip>

      <Tooltip content="Bottom tooltip" placement="bottom">
        <Button>Bottom</Button>
      </Tooltip>

      <Tooltip content="Left tooltip" placement="left">
        <Button>Left</Button>
      </Tooltip>
    </div>
  ),
};

export const WithText: StoryObj = {
  render: () => (
    <div
      style={{
        display: "flex",
        gap: "32px",
        alignItems: "center",
        padding: "40px",
      }}
    >
      <Tooltip content="This is a longer tooltip with more text content">
        <Text>Hover for long tooltip</Text>
      </Tooltip>

      <Tooltip
        content={
          <div>
            <div style={{ fontWeight: "bold", marginBottom: "4px" }}>
              Rich Content
            </div>
            <div>This tooltip contains HTML content</div>
          </div>
        }
      >
        <Text>Rich tooltip</Text>
      </Tooltip>
    </div>
  ),
};

export const Disabled: StoryObj = {
  render: () => (
    <div
      style={{
        display: "flex",
        gap: "32px",
        alignItems: "center",
        padding: "40px",
      }}
    >
      <Tooltip content="This tooltip is disabled" disabled>
        <Button>Disabled tooltip</Button>
      </Tooltip>

      <Tooltip content="This tooltip has a delay" delay={1000}>
        <Button>Delayed tooltip</Button>
      </Tooltip>
    </div>
  ),
};
