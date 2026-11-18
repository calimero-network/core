import type { Meta, StoryObj } from "@storybook/react-vite";

const SemanticColorsDemo = () => (
  <div
    style={{
      padding: "40px",
      fontFamily: "Power Grotesk, system-ui, sans-serif",
    }}
  >
    <h1
      style={{
        fontSize: "32px",
        fontWeight: "700",
        marginBottom: "32px",
        color: "#333",
      }}
    >
      🎨 Semantic Colors Demo
    </h1>

    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fit, minmax(200px, 1fr))",
        gap: "24px",
      }}
    >
      {/* Success */}
      <div
        style={{
          padding: "24px",
          borderRadius: "12px",
          backgroundColor: "var(--color-semantic-success)",
          color: "white",
          textAlign: "center",
        }}
      >
        <h3 style={{ margin: "0 0 12px 0", fontSize: "20px" }}>Success</h3>
        <p style={{ margin: "0", fontSize: "14px", opacity: "0.9" }}>#16a34a</p>
        <p style={{ margin: "8px 0 0 0", fontSize: "12px" }}>
          Used for positive states, confirmations, and successful actions
        </p>
      </div>

      {/* Warning */}
      <div
        style={{
          padding: "24px",
          borderRadius: "12px",
          backgroundColor: "var(--color-semantic-warning)",
          color: "white",
          textAlign: "center",
        }}
      >
        <h3 style={{ margin: "0 0 12px 0", fontSize: "20px" }}>Warning</h3>
        <p style={{ margin: "0", fontSize: "14px", opacity: "0.9" }}>#f59e0b</p>
        <p style={{ margin: "8px 0 0 0", fontSize: "12px" }}>
          Used for caution states, alerts, and attention-grabbing elements
        </p>
      </div>

      {/* Error */}
      <div
        style={{
          padding: "24px",
          borderRadius: "12px",
          backgroundColor: "var(--color-semantic-error)",
          color: "white",
          textAlign: "center",
        }}
      >
        <h3 style={{ margin: "0 0 12px 0", fontSize: "20px" }}>Error</h3>
        <p style={{ margin: "0", fontSize: "14px", opacity: "0.9" }}>#ef4444</p>
        <p style={{ margin: "8px 0 0 0", fontSize: "12px" }}>
          Used for error states, failures, and critical issues
        </p>
      </div>

      {/* Info */}
      <div
        style={{
          padding: "24px",
          borderRadius: "12px",
          backgroundColor: "var(--color-semantic-info)",
          color: "white",
          textAlign: "center",
        }}
      >
        <h3 style={{ margin: "0 0 12px 0", fontSize: "20px" }}>Info</h3>
        <p style={{ margin: "0", fontSize: "14px", opacity: "0.9" }}>#3b82f6</p>
        <p style={{ margin: "8px 0 0 0", fontSize: "12px" }}>
          Used for informational states, links, and neutral actions
        </p>
      </div>
    </div>

    <div
      style={{
        marginTop: "40px",
        padding: "24px",
        backgroundColor: "var(--color-background-primary)",
        borderRadius: "12px",
      }}
    >
      <h2 style={{ color: "white", margin: "0 0 16px 0", fontSize: "24px" }}>
        Usage in Charts
      </h2>
      <p
        style={{
          color: "var(--color-neutral-400)",
          margin: "0",
          fontSize: "16px",
        }}
      >
        These semantic colors are used throughout the charts package for
        consistent theming:
      </p>
      <ul
        style={{
          color: "var(--color-neutral-400)",
          margin: "16px 0 0 0",
          paddingLeft: "24px",
        }}
      >
        <li>
          <strong style={{ color: "var(--color-semantic-success)" }}>
            Success:
          </strong>{" "}
          Gauge good zones, positive trends
        </li>
        <li>
          <strong style={{ color: "var(--color-semantic-warning)" }}>
            Warning:
          </strong>{" "}
          Gauge warning zones, caution indicators
        </li>
        <li>
          <strong style={{ color: "var(--color-semantic-error)" }}>
            Error:
          </strong>{" "}
          Gauge critical zones, error states
        </li>
        <li>
          <strong style={{ color: "var(--color-semantic-info)" }}>Info:</strong>{" "}
          Neutral data points, informational elements
        </li>
      </ul>
    </div>
  </div>
);

const meta: Meta<typeof SemanticColorsDemo> = {
  title: "Foundation/Semantic Colors",
  component: SemanticColorsDemo,
  parameters: {
    layout: "fullscreen",
  },
};

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
