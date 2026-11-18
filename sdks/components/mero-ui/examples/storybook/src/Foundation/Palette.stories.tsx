import type { Meta, StoryObj } from "@storybook/react-vite";

const ColorSwatch = ({
  color,
  name,
  value,
}: {
  color: string;
  name: string;
  value: string;
}) => (
  <div
    style={{
      display: "flex",
      flexDirection: "column",
      alignItems: "center",
      margin: "8px",
      borderRadius: "8px",
      overflow: "hidden",
      boxShadow: "0 2px 8px rgba(0,0,0,0.1)",
    }}
  >
    <div
      style={{
        width: "80px",
        height: "80px",
        backgroundColor: color,
        border: "1px solid #e0e0e0",
      }}
    />
    <div
      style={{
        padding: "8px",
        textAlign: "center",
        backgroundColor: "white",
        width: "100%",
      }}
    >
      <div style={{ fontSize: "12px", fontWeight: "bold", color: "#333" }}>
        {name}
      </div>
      <div style={{ fontSize: "10px", color: "#666", fontFamily: "monospace" }}>
        {value}
      </div>
    </div>
  </div>
);

const Palette = () => (
  <div
    style={{
      padding: "40px",
      fontFamily: "Power Grotesk, system-ui, sans-serif",
    }}
  >
    {/* Banner */}
    <div
      style={{
        background:
          "linear-gradient(135deg, var(--color-brand-600) 0%, var(--color-background-brand) 100%)",
        color: "#0A0A0A",
        padding: "24px",
        borderRadius: "12px",
        marginBottom: "40px",
        textAlign: "center",
      }}
    >
      <h1 style={{ margin: "0 0 16px 0", fontSize: "32px", fontWeight: "700" }}>
        🎨 Calimero Design System
      </h1>
      <p style={{ margin: "0", fontSize: "18px", opacity: "0.9" }}>
        Solid, consistent background colors for better usability
      </p>
    </div>

    {/* Brand Colors */}
    <div style={{ marginBottom: "40px" }}>
      <h2
        style={{
          fontSize: "24px",
          fontWeight: "600",
          marginBottom: "20px",
          color: "#333",
        }}
      >
        Brand Colors (5)
      </h2>
      <div style={{ display: "flex", flexWrap: "wrap", gap: "8px" }}>
        <ColorSwatch
          color="var(--color-brand-900)"
          name="brand-900"
          value="#2D381B"
        />
        <ColorSwatch
          color="var(--color-brand-800)"
          name="brand-800"
          value="#8AA200"
        />
        <ColorSwatch
          color="var(--color-brand-700)"
          name="brand-700"
          value="#73B30C"
        />
        <ColorSwatch
          color="var(--color-brand-600)"
          name="brand-600"
          value="#A5FF11"
        />
        <ColorSwatch
          color="var(--color-brand-100)"
          name="brand-100"
          value="#ECFC91"
        />
      </div>
    </div>

    {/* Neutral Colors */}
    <div style={{ marginBottom: "40px" }}>
      <h2
        style={{
          fontSize: "24px",
          fontWeight: "600",
          marginBottom: "20px",
          color: "#333",
        }}
      >
        Neutral Colors (7)
      </h2>
      <div style={{ display: "flex", flexWrap: "wrap", gap: "8px" }}>
        <ColorSwatch
          color="var(--color-neutral-900)"
          name="neutral-900"
          value="#131215"
        />
        <ColorSwatch
          color="var(--color-neutral-800)"
          name="neutral-800"
          value="#222222"
        />
        <ColorSwatch
          color="var(--color-neutral-700)"
          name="neutral-700"
          value="#282828"
        />
        <ColorSwatch
          color="var(--color-neutral-600)"
          name="neutral-600"
          value="#404040"
        />
        <ColorSwatch
          color="var(--color-neutral-400)"
          name="neutral-400"
          value="#8E8E8E"
        />
        <ColorSwatch
          color="var(--color-neutral-300)"
          name="neutral-300"
          value="#A0A0A0"
        />
        <ColorSwatch
          color="var(--color-neutral-200)"
          name="neutral-200"
          value="#C8C8C8"
        />
      </div>
    </div>

    {/* Background Colors */}
    <div style={{ marginBottom: "40px" }}>
      <h2
        style={{
          fontSize: "24px",
          fontWeight: "600",
          marginBottom: "20px",
          color: "#333",
        }}
      >
        Background Colors (4)
      </h2>
      <div style={{ display: "flex", flexWrap: "wrap", gap: "8px" }}>
        <ColorSwatch
          color="var(--color-background-primary)"
          name="background-primary"
          value="#1A1A1A"
        />
        <ColorSwatch
          color="var(--color-background-secondary)"
          name="background-secondary"
          value="#2A2A2A"
        />
        <ColorSwatch
          color="var(--color-background-tertiary)"
          name="background-tertiary"
          value="#3A3A3A"
        />
        <ColorSwatch
          color="var(--color-background-brand)"
          name="background-brand"
          value="#1F2A15"
        />
      </div>
    </div>

    {/* Semantic Colors */}
    <div style={{ marginBottom: "40px" }}>
      <h2
        style={{
          fontSize: "24px",
          fontWeight: "600",
          marginBottom: "20px",
          color: "#333",
        }}
      >
        Semantic Colors (4)
      </h2>
      <div style={{ display: "flex", flexWrap: "wrap", gap: "8px" }}>
        <ColorSwatch
          color="var(--color-semantic-success)"
          name="semantic-success"
          value="#16a34a"
        />
        <ColorSwatch
          color="var(--color-semantic-warning)"
          name="semantic-warning"
          value="#f59e0b"
        />
        <ColorSwatch
          color="var(--color-semantic-error)"
          name="semantic-error"
          value="#ef4444"
        />
        <ColorSwatch
          color="var(--color-semantic-info)"
          name="semantic-info"
          value="#3b82f6"
        />
      </div>
    </div>
  </div>
);

const meta: Meta<typeof Palette> = {
  title: "Foundation/Palette",
  component: Palette,
  parameters: {
    layout: "fullscreen",
  },
};

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
