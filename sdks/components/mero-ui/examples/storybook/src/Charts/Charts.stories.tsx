import type { Meta, StoryObj } from "@storybook/react-vite";
import { Gauge } from "@calimero-network/mero-charts";
import {
  Card,
  CardHeader,
  CardTitle,
  CardContent,
} from "@calimero-network/mero-ui";

const meta: Meta<typeof Gauge> = {
  title: "Charts/Charts",
  component: Gauge,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
};

export default meta;

export const GaugeExample: StoryObj<typeof Gauge> = {
  render: (args) => {
    // Update value if it's outside the min/max range
    const adjustedValue = Math.max(
      args.min || 0,
      Math.min(args.max || 100, args.value),
    );

    return (
      <div style={{ padding: "16px" }}>
        <div style={{ maxWidth: "400px", margin: "0 auto" }}>
          <Card>
            <CardHeader>
              <CardTitle>CPU Usage</CardTitle>
            </CardHeader>
            <CardContent>
              <Gauge {...args} value={adjustedValue} />
            </CardContent>
          </Card>
        </div>
        {/* Show current range info */}
        <div
          style={{
            marginTop: "16px",
            fontSize: "14px",
            color: "#9ca3af",
            textAlign: "center",
          }}
        >
          <p>
            Current range: {args.min} to {args.max}
          </p>
          <p>Value: {adjustedValue} (clamped to range)</p>
        </div>
      </div>
    );
  },
  args: {
    value: 66.8,
    min: 0,
    max: 100,
    thresholds: { warning: 60, danger: 80 },
    label: "%",
    decimals: 1,
  },
  argTypes: {
    value: {
      control: "text",
      description: "Current value to display",
    },
    min: {
      control: "text",
      description: "Minimum value",
    },
    max: {
      control: "text",
      description: "Maximum value",
    },
    label: {
      control: "text",
      description: 'Value label (e.g., "%")',
    },
    decimals: {
      control: "text",
      description: "Number of decimal places",
    },
  },
};

// Comprehensive story showing all prop combinations
export const GaugeAllProps: StoryObj<typeof Gauge> = {
  render: () => {
    return (
      <div style={{ padding: "24px" }}>
        <h2
          style={{
            fontSize: "24px",
            fontWeight: "bold",
            color: "white",
            marginBottom: "24px",
          }}
        >
          Gauge Component - All Prop Combinations
        </h2>

        {/* Different sizes */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            Different Sizes
          </h3>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fit, minmax(200px, 1fr))",
              gap: "16px",
            }}
          >
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                Small
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>CPU Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={75} label="%" />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                Medium (default)
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>CPU Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={75} label="%" />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                Large
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>CPU Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={75} label="%" />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                Extra Large
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>CPU Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={75} label="%" />
                </CardContent>
              </Card>
            </div>
          </div>
        </div>

        {/* Different value states */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            Different Value States
          </h3>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fit, minmax(250px, 1fr))",
              gap: "16px",
            }}
          >
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                Good (&le;60%)
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>Memory Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge
                    value={45}
                    label="%"
                    thresholds={{ warning: 60, danger: 80 }}
                  />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                Warning (&le;80%)
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>Memory Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge
                    value={70}
                    label="%"
                    thresholds={{ warning: 60, danger: 80 }}
                  />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                Danger (&gt;80%)
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>Memory Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge
                    value={95}
                    label="%"
                    thresholds={{ warning: 60, danger: 80 }}
                  />
                </CardContent>
              </Card>
            </div>
          </div>
        </div>

        {/* Different ranges */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            Different Ranges
          </h3>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fit, minmax(250px, 1fr))",
              gap: "16px",
            }}
          >
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                0-100 (default)
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>CPU Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={75} label="%" min={0} max={100} />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                -10 to 10
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>Temperature</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={3} label="°C" min={-10} max={10} />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                0 to 1000
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>Requests/sec</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={750} label="req/s" min={0} max={1000} />
                </CardContent>
              </Card>
            </div>
          </div>
        </div>

        {/* Different decimal places */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            Different Decimal Places
          </h3>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fit, minmax(200px, 1fr))",
              gap: "16px",
            }}
          >
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                0 decimals
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>CPU Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={75.6} label="%" decimals={0} />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                1 decimal (default)
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>CPU Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={75.6} label="%" decimals={1} />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                2 decimals
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>CPU Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={75.6} label="%" decimals={2} />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                3 decimals
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>CPU Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={75.6} label="%" decimals={3} />
                </CardContent>
              </Card>
            </div>
          </div>
        </div>

        {/* Different thresholds */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            Different Thresholds
          </h3>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fit, minmax(250px, 1fr))",
              gap: "16px",
            }}
          >
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                Conservative (30/70)
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>Disk Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge
                    value={50}
                    label="%"
                    thresholds={{ warning: 30, danger: 70 }}
                  />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                Standard (60/80)
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>Disk Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge
                    value={50}
                    label="%"
                    thresholds={{ warning: 60, danger: 80 }}
                  />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                Aggressive (80/95)
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>Disk Usage</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge
                    value={50}
                    label="%"
                    thresholds={{ warning: 80, danger: 95 }}
                  />
                </CardContent>
              </Card>
            </div>
          </div>
        </div>

        {/* Long titles */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            Long Titles (with ellipsis)
          </h3>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))",
              gap: "16px",
            }}
          >
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                Very Long Title
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>
                    This is a very long title that should be truncated with
                    ellipsis when it exceeds the available space
                  </CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={75} label="%" />
                </CardContent>
              </Card>
            </div>
            <div style={{ textAlign: "center" }}>
              <p
                style={{
                  fontSize: "14px",
                  color: "#9ca3af",
                  marginBottom: "8px",
                }}
              >
                Short Title
              </p>
              <Card>
                <CardHeader>
                  <CardTitle>Short</CardTitle>
                </CardHeader>
                <CardContent>
                  <Gauge value={75} label="%" />
                </CardContent>
              </Card>
            </div>
          </div>
        </div>
      </div>
    );
  },
};
