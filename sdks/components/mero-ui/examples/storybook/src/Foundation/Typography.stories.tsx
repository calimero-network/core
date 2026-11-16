import type { Meta, StoryObj } from "@storybook/react-vite";
import { Text, Heading, Code, Link } from "@calimero-network/mero-ui";

const meta: Meta = {
  title: "Foundation/Typography",
  parameters: {
    layout: "padded",
  },
};

export default meta;

export const TextVariants: StoryObj = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <Text size="xs">Extra small text</Text>
      <Text size="sm">Small text</Text>
      <Text size="md">Medium text</Text>
      <Text size="lg">Large text</Text>
      <Text size="xl">Extra large text</Text>
    </div>
  ),
};

export const TextWeights: StoryObj = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "8px" }}>
      <Text weight="normal">Normal weight text</Text>
      <Text weight="medium">Medium weight text</Text>
      <Text weight="semibold">Semibold weight text</Text>
      <Text weight="bold">Bold weight text</Text>
    </div>
  ),
};

export const TextColors: StoryObj = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "8px" }}>
      <Text color="primary">Primary text</Text>
      <Text color="secondary">Secondary text</Text>
      <Text color="muted">Muted text</Text>
      <Text color="success">Success text</Text>
      <Text color="warning">Warning text</Text>
      <Text color="error">Error text</Text>
    </div>
  ),
};

export const Headings: StoryObj = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <Heading level={1}>Heading 1</Heading>
      <Heading level={2}>Heading 2</Heading>
      <Heading level={3}>Heading 3</Heading>
      <Heading level={4}>Heading 4</Heading>
      <Heading level={5}>Heading 5</Heading>
      <Heading level={6}>Heading 6</Heading>
    </div>
  ),
};

export const CodeExamples: StoryObj = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <div>
        <Text>
          Inline code: <Code>const example = "hello world"</Code>
        </Text>
      </div>
      <Code variant="block" language="javascript">
        {`function greet(name) {
  return \`Hello, \${name}!\`;
}

console.log(greet("World"));`}
      </Code>
    </div>
  ),
};

export const Links: StoryObj = {
  render: () => (
    <div style={{ display: "flex", flexDirection: "column", gap: "16px" }}>
      <Link href="#" variant="default">
        Default link
      </Link>
      <Link href="#" variant="subtle">
        Subtle link
      </Link>
      <Link href="#" variant="underline">
        Underline link
      </Link>
      <Link href="#" color="success">
        Success link
      </Link>
      <Link href="#" color="warning">
        Warning link
      </Link>
      <Link href="#" color="error">
        Error link
      </Link>
      <Link href="#" disabled>
        Disabled link
      </Link>
    </div>
  ),
};
