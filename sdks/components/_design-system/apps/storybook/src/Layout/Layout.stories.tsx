import type { Meta, StoryObj } from "@storybook/react-vite";
import { Layout, AuthLayout, Container } from "@calimero-network/mero-ui";

const meta: Meta<typeof Layout> = {
  title: "Layout/Layout",
  component: Layout,
  parameters: {
    layout: "fullscreen",
  },
  argTypes: {
    maxWidth: {
      control: { type: "text" },
      description: "Maximum width of the layout container",
    },
    padding: {
      control: { type: "text" },
      description: "Padding around the content",
    },
    centered: {
      control: { type: "boolean" },
      description: "Whether to center the content",
    },
    variant: {
      control: { type: "select" },
      options: ["auth", "default"],
      description: "Layout variant",
    },
  },
};

export default meta;
type Story = StoryObj<typeof Layout>;

export const Default: Story = {
  args: {
    children: (
      <div
        style={{
          padding: "20px",
          background: "var(--color-background-secondary)",
          borderRadius: "8px",
        }}
      >
        <h2>Default Layout</h2>
        <p>This is a default layout container with centered content.</p>
      </div>
    ),
    maxWidth: "600px",
    padding: "24px",
    centered: true,
    variant: "default",
  },
};

export const AuthLayoutStory: Story = {
  render: (args) => (
    <AuthLayout {...args}>
      <div style={{ textAlign: "center" }}>
        <h1 style={{ color: "white", marginBottom: "16px" }}>Welcome Back</h1>
        <p style={{ color: "var(--color-neutral-300)", marginBottom: "24px" }}>
          Sign in to your account to continue
        </p>
        <div style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
          <input
            type="email"
            placeholder="Email"
            style={{
              padding: "12px",
              borderRadius: "8px",
              border: "1px solid var(--color-neutral-600)",
              background: "var(--color-background-primary)",
              color: "white",
              fontSize: "14px",
            }}
          />
          <input
            type="password"
            placeholder="Password"
            style={{
              padding: "12px",
              borderRadius: "8px",
              border: "1px solid var(--color-neutral-600)",
              background: "var(--color-background-primary)",
              color: "white",
              fontSize: "14px",
            }}
          />
          <button
            style={{
              padding: "12px",
              borderRadius: "8px",
              background: "var(--color-brand-600)",
              color: "black",
              border: "none",
              fontSize: "14px",
              fontWeight: "600",
              cursor: "pointer",
            }}
          >
            Sign In
          </button>
        </div>
      </div>
    </AuthLayout>
  ),
  args: {
    maxWidth: "400px",
    padding: "32px",
  },
};

export const ContainerStory: Story = {
  render: (args) => (
    <Container {...args}>
      <div
        style={{
          padding: "20px",
          background: "var(--color-background-secondary)",
          borderRadius: "8px",
        }}
      >
        <h2>Container Layout</h2>
        <p>
          This is a container layout with a larger max width, suitable for page
          content.
        </p>
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fit, minmax(200px, 1fr))",
            gap: "16px",
            marginTop: "16px",
          }}
        >
          <div
            style={{
              padding: "16px",
              background: "var(--color-background-tertiary)",
              borderRadius: "8px",
            }}
          >
            <h3>Card 1</h3>
            <p>Content here</p>
          </div>
          <div
            style={{
              padding: "16px",
              background: "var(--color-background-tertiary)",
              borderRadius: "8px",
            }}
          >
            <h3>Card 2</h3>
            <p>Content here</p>
          </div>
          <div
            style={{
              padding: "16px",
              background: "var(--color-background-tertiary)",
              borderRadius: "8px",
            }}
          >
            <h3>Card 3</h3>
            <p>Content here</p>
          </div>
        </div>
      </div>
    </Container>
  ),
  args: {
    maxWidth: "1200px",
    padding: "16px",
  },
};

export const NotCentered: Story = {
  args: {
    children: (
      <div
        style={{
          padding: "20px",
          background: "var(--color-background-secondary)",
          borderRadius: "8px",
        }}
      >
        <h2>Not Centered Layout</h2>
        <p>This layout is not centered and flows naturally.</p>
      </div>
    ),
    maxWidth: "800px",
    padding: "16px",
    centered: false,
    variant: "default",
  },
};
