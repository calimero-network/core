import type { Meta, StoryObj } from "@storybook/react-vite";
import { Popover, Button, Text, Box, Stack } from "@calimero-network/mero-ui";
import React from "react";

const meta: Meta<typeof Popover> = {
  title: "Feedback/Popover",
  component: Popover,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
};

export default meta;
type Story = StoryObj<typeof Popover>;

export const Default: Story = {
  render: () => (
    <Popover
      content={
        <Box padding="md">
          <Text>This is a popover content</Text>
        </Box>
      }
    >
      <Button>Open Popover</Button>
    </Popover>
  ),
};

export const WithRichContent: Story = {
  render: () => (
    <Popover
      content={
        <Box padding="lg" style={{ minWidth: "200px" }}>
          <Stack gap="sm">
            <Text weight="bold">Settings</Text>
            <Text size="sm">Configure your preferences</Text>
            <Button size="sm" variant="primary">
              Save Changes
            </Button>
          </Stack>
        </Box>
      }
    >
      <Button variant="outline">Settings</Button>
    </Popover>
  ),
};

export const DifferentPlacements: Story = {
  render: () => (
    <Box style={{ display: "flex", gap: "16px", flexWrap: "wrap" }}>
      <Popover
        placement="top"
        content={
          <Box padding="md">
            <Text>Top placement</Text>
          </Box>
        }
      >
        <Button>Top</Button>
      </Popover>

      <Popover
        placement="bottom"
        content={
          <Box padding="md">
            <Text>Bottom placement</Text>
          </Box>
        }
      >
        <Button>Bottom</Button>
      </Popover>

      <Popover
        placement="left"
        content={
          <Box padding="md">
            <Text>Left placement</Text>
          </Box>
        }
      >
        <Button>Left</Button>
      </Popover>

      <Popover
        placement="right"
        content={
          <Box padding="md">
            <Text>Right placement</Text>
          </Box>
        }
      >
        <Button>Right</Button>
      </Popover>
    </Box>
  ),
};

export const HoverTrigger: Story = {
  render: () => (
    <Popover
      trigger="hover"
      content={
        <Box padding="md">
          <Text>This popover appears on hover</Text>
        </Box>
      }
    >
      <Button variant="outline">Hover me</Button>
    </Popover>
  ),
};

export const FocusTrigger: Story = {
  render: () => (
    <Popover
      trigger="focus"
      content={
        <Box padding="md">
          <Text>This popover appears on focus</Text>
        </Box>
      }
    >
      <Button variant="outline">Focus me</Button>
    </Popover>
  ),
};

export const Disabled: Story = {
  render: () => (
    <Popover
      disabled
      content={
        <Box padding="md">
          <Text>This popover is disabled</Text>
        </Box>
      }
    >
      <Button disabled>Disabled Popover</Button>
    </Popover>
  ),
};
