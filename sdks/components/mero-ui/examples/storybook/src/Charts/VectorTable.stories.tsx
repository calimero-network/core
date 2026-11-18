import type { Meta, StoryObj } from "@storybook/react-vite";
import { VectorTableRaw } from "@calimero-network/mero-charts";
import {
  Card,
  CardHeader,
  CardTitle,
  CardContent,
} from "@calimero-network/mero-ui";
import {
  calculateMean,
  calculateLast,
  calculateMax,
  type Series,
} from "@calimero-network/mero-charts";

const meta: Meta<typeof VectorTableRaw> = {
  title: "Charts/Vector Table",
  component: VectorTableRaw,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
};

export default meta;

// Sample data for vector table
const sampleVectorData = [
  {
    name: "10.0.99.165:8429 - /api/v1/query",
    mean: 11.7,
    last: 12.1,
    max: 12.1,
  },
  {
    name: "10.0.99.165:8429 - /api/v1/write",
    mean: 2.69,
    last: 2.75,
    max: 2.75,
  },
  {
    name: "10.0.99.165:8429 - /api/v1/query_range",
    mean: 0.408,
    last: 0.467,
    max: 0.467,
  },
];

const errorVectorData = [
  {
    name: "10.0.99.165:8429 - /api/v1/query",
    mean: 0.0,
    last: 0.0,
    max: 0.0,
  },
  {
    name: "10.0.83.199:8429 - /api/v1/write",
    mean: 0.0,
    last: 0.0,
    max: 0.0,
  },
  {
    name: "10.0.77.25:8080 - /api/v1/query_range",
    mean: 0.0,
    last: 0.0,
    max: 0.0,
  },
];

export const VectorTableExample: StoryObj<typeof VectorTableRaw> = {
  render: (args) => {
    return (
      <div style={{ padding: "16px" }}>
        <div style={{ maxWidth: "800px", margin: "0 auto" }}>
          <Card>
            <CardHeader>
              <CardTitle>API Request Rate</CardTitle>
            </CardHeader>
            <CardContent>
              <VectorTableRaw {...args} />
            </CardContent>
          </Card>
        </div>
      </div>
    );
  },
  args: {
    data: sampleVectorData,
  },
  argTypes: {
    data: {
      control: "object",
      description: "Table data",
    },
    colors: {
      control: "object",
      description: "Array of colors for each row",
    },
  },
};

export const VectorTableWithColors: StoryObj<typeof VectorTableRaw> = {
  render: (args) => {
    return (
      <div style={{ padding: "16px" }}>
        <div style={{ maxWidth: "800px", margin: "0 auto" }}>
          <Card>
            <CardHeader>
              <CardTitle>API Request Rate</CardTitle>
            </CardHeader>
            <CardContent>
              <VectorTableRaw {...args} />
            </CardContent>
          </Card>
        </div>
      </div>
    );
  },
  args: {
    data: sampleVectorData,
    colors: ["#fbbf24", "#f59e0b", "#3b82f6"],
  },
};

export const VectorTableAllProps: StoryObj<typeof VectorTableRaw> = {
  render: () => {
    return (
      <div style={{ padding: "24px", maxWidth: "1200px", margin: "0 auto" }}>
        <h2
          style={{
            fontSize: "24px",
            fontWeight: "bold",
            color: "white",
            marginBottom: "24px",
          }}
        >
          Vector Table Component - All Variations
        </h2>

        {/* Default columns */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            Default Columns (Mean, Last *, Max)
          </h3>
          <Card>
            <CardHeader>
              <CardTitle>API Performance Metrics</CardTitle>
            </CardHeader>
            <CardContent>
              <VectorTableRaw data={sampleVectorData} />
            </CardContent>
          </Card>
        </div>

        {/* With custom colors */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            With Custom Colors
          </h3>
          <Card>
            <CardHeader>
              <CardTitle>API Performance Metrics</CardTitle>
            </CardHeader>
            <CardContent>
              <VectorTableRaw
                data={sampleVectorData}
                colors={["#A5FF11", "#fbbf24", "#3b82f6"]}
              />
            </CardContent>
          </Card>
        </div>

        {/* Error state */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            Error State (Zero Values)
          </h3>
          <Card>
            <CardHeader>
              <CardTitle>API Performance Metrics (Error)</CardTitle>
            </CardHeader>
            <CardContent>
              <VectorTableRaw
                data={errorVectorData}
                colors={["#ef4444", "#f97316", "#f59e0b"]}
              />
            </CardContent>
          </Card>
        </div>
      </div>
    );
  },
};

export const VectorTableCustomColumns: StoryObj<typeof VectorTableRaw> = {
  render: () => {
    // Custom column configuration
    const customColumns = [
      { title: "Average", function: calculateMean },
      { title: "Current", function: calculateLast },
      { title: "Peak", function: calculateMax },
      {
        title: "Min",
        function: (series: Series) => Math.min(...series.data.map((p) => p.y)),
      },
    ];

    return (
      <div style={{ padding: "16px" }}>
        <div style={{ maxWidth: "800px", margin: "0 auto" }}>
          <Card>
            <CardHeader>
              <CardTitle>Custom Column Configuration</CardTitle>
            </CardHeader>
            <CardContent>
              <VectorTableRaw
                data={sampleVectorData}
                colors={["#A5FF11", "#fbbf24", "#3b82f6"]}
                columns={customColumns}
              />
            </CardContent>
          </Card>
        </div>
      </div>
    );
  },
};
