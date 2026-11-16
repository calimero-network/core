import type { Meta, StoryObj } from "@storybook/react-vite";
import { Box, Text } from "@calimero-network/mero-ui";

const meta: Meta = {
  title: "Layout/Box",
  parameters: {
    layout: "padded",
  },
};

export default meta;

export const Basic: StoryObj = {
  render: () => (
    <Box padding="md" backgroundColor="muted" borderRadius="md">
      <Text>This is a basic box with padding and background</Text>
    </Box>
  ),
};

export const Sizes: StoryObj = {
  render: () => (
    <div style={{ display: "flex", gap: "16px", alignItems: "flex-start" }}>
      <Box padding="sm" backgroundColor="primary" borderRadius="sm">
        <Text color="inverse" size="sm">
          Small
        </Text>
      </Box>
      <Box padding="md" backgroundColor="primary" borderRadius="md">
        <Text color="inverse">Medium</Text>
      </Box>
      <Box padding="lg" backgroundColor="primary" borderRadius="lg">
        <Text color="inverse" size="lg">
          Large
        </Text>
      </Box>
    </div>
  ),
};

export const Flexbox: StoryObj = {
  render: () => (
    <Box
      display="flex"
      gap="md"
      padding="lg"
      backgroundColor="muted"
      borderRadius="md"
      style={{ height: "200px" }}
    >
      <Box
        padding="md"
        backgroundColor="primary"
        borderRadius="md"
        style={{ flex: 1 }}
      >
        <Text color="inverse">Flex 1</Text>
      </Box>
      <Box
        padding="md"
        backgroundColor="success"
        borderRadius="md"
        style={{ flex: 2 }}
      >
        <Text color="inverse">Flex 2</Text>
      </Box>
      <Box
        padding="md"
        backgroundColor="warning"
        borderRadius="md"
        style={{ flex: 1 }}
      >
        <Text color="inverse">Flex 1</Text>
      </Box>
    </Box>
  ),
};

export const Shadows: StoryObj = {
  render: () => (
    <div style={{ display: "flex", gap: "24px", alignItems: "center" }}>
      <Box padding="lg" backgroundColor="primary" borderRadius="md" shadow="sm">
        <Text color="inverse">Small shadow</Text>
      </Box>
      <Box padding="lg" backgroundColor="primary" borderRadius="md" shadow="md">
        <Text color="inverse">Medium shadow</Text>
      </Box>
      <Box padding="lg" backgroundColor="primary" borderRadius="md" shadow="lg">
        <Text color="inverse">Large shadow</Text>
      </Box>
    </div>
  ),
};

export const Borders: StoryObj = {
  render: () => (
    <div style={{ display: "flex", gap: "16px", alignItems: "center" }}>
      <Box
        padding="md"
        backgroundColor="primary"
        borderRadius="md"
        border="thin"
        borderColor="default"
      >
        <Text color="inverse">Thin border</Text>
      </Box>
      <Box
        padding="md"
        backgroundColor="primary"
        borderRadius="md"
        border="medium"
        borderColor="success"
      >
        <Text color="inverse">Medium border</Text>
      </Box>
      <Box
        padding="md"
        backgroundColor="primary"
        borderRadius="md"
        border="thick"
        borderColor="error"
      >
        <Text color="inverse">Thick border</Text>
      </Box>
    </div>
  ),
};
