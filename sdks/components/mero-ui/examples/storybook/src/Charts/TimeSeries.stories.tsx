import type { Meta, StoryObj } from "@storybook/react-vite";
import { TimeSeries } from "@calimero-network/mero-charts";
import {
  Card,
  CardHeader,
  CardTitle,
  CardContent,
} from "@calimero-network/mero-ui";

const meta: Meta<typeof TimeSeries> = {
  title: "Charts/Time Series",
  component: TimeSeries,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
};

export default meta;

// Sample data for time series
const sampleTimeSeriesData = [
  {
    name: "CPU Usage",
    data: [
      { t: Date.now() - 60000 * 5, y: 45 },
      { t: Date.now() - 60000 * 4, y: 52 },
      { t: Date.now() - 60000 * 3, y: 48 },
      { t: Date.now() - 60000 * 2, y: 67 },
      { t: Date.now() - 60000 * 1, y: 73 },
      { t: Date.now(), y: 68 },
    ],
    fill: true,
  },
  {
    name: "Memory Usage",
    data: [
      { t: Date.now() - 60000 * 5, y: 62 },
      { t: Date.now() - 60000 * 4, y: 65 },
      { t: Date.now() - 60000 * 3, y: 58 },
      { t: Date.now() - 60000 * 2, y: 71 },
      { t: Date.now() - 60000 * 1, y: 69 },
      { t: Date.now(), y: 75 },
    ],
    fill: false,
  },
];

const singleSeriesData = [
  {
    name: "Network Traffic",
    data: [
      { t: Date.now() - 60000 * 5, y: 2.1 },
      { t: Date.now() - 60000 * 4, y: 2.8 },
      { t: Date.now() - 60000 * 3, y: 2.3 },
      { t: Date.now() - 60000 * 2, y: 3.2 },
      { t: Date.now() - 60000 * 1, y: 2.9 },
      { t: Date.now(), y: 3.1 },
    ],
    fill: true,
  },
];

export const TimeSeriesExample: StoryObj<typeof TimeSeries> = {
  render: (args) => {
    return (
      <div style={{ padding: "16px" }}>
        <div style={{ maxWidth: "800px", margin: "0 auto" }}>
          <Card>
            <CardHeader>
              <CardTitle>System Metrics</CardTitle>
            </CardHeader>
            <CardContent>
              <div style={{ height: "240px" }}>
                <TimeSeries {...args} />
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    );
  },
  args: {
    series: sampleTimeSeriesData,
    yLabel: "Percentage",
  },
  argTypes: {
    yLabel: {
      control: "text",
      description: "Y-axis label",
    },
    colors: {
      control: "object",
      description: "Array of colors for each series",
    },
    showLegend: {
      control: "boolean",
      description: "Show/hide the legend",
    },
  },
};

export const TimeSeriesSingleSeries: StoryObj<typeof TimeSeries> = {
  render: (args) => {
    return (
      <div style={{ padding: "16px" }}>
        <div style={{ maxWidth: "800px", margin: "0 auto" }}>
          <Card>
            <CardHeader>
              <CardTitle>Network Traffic</CardTitle>
            </CardHeader>
            <CardContent>
              <div style={{ height: "240px" }}>
                <TimeSeries {...args} />
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    );
  },
  args: {
    series: singleSeriesData,
    yLabel: "MB/s",
  },
};

export const TimeSeriesAllProps: StoryObj<typeof TimeSeries> = {
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
          Time Series Component - All Variations
        </h2>

        {/* Multiple series with legend */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            Multiple Series with Legend
          </h3>
          <Card>
            <CardHeader>
              <CardTitle>System Performance Metrics</CardTitle>
            </CardHeader>
            <CardContent>
              <div style={{ height: "240px" }}>
                <TimeSeries
                  series={sampleTimeSeriesData}
                  yLabel="Percentage"
                  showLegend={true}
                />
              </div>
            </CardContent>
          </Card>
        </div>

        {/* Single series without legend */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            Single Series without Legend
          </h3>
          <Card>
            <CardHeader>
              <CardTitle>Network Traffic</CardTitle>
            </CardHeader>
            <CardContent>
              <div style={{ height: "240px" }}>
                <TimeSeries
                  series={singleSeriesData}
                  yLabel="MB/s"
                  showLegend={false}
                />
              </div>
            </CardContent>
          </Card>
        </div>

        {/* Custom colors */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            Custom Colors
          </h3>
          <Card>
            <CardHeader>
              <CardTitle>Custom Colored Metrics</CardTitle>
            </CardHeader>
            <CardContent>
              <div style={{ height: "240px" }}>
                <TimeSeries
                  series={sampleTimeSeriesData}
                  yLabel="Percentage"
                  colors={["#A5FF11", "#fbbf24"]}
                  showLegend={true}
                />
              </div>
            </CardContent>
          </Card>
        </div>

        {/* Raw component without card */}
        <div style={{ marginBottom: "32px" }}>
          <h3
            style={{
              fontSize: "18px",
              fontWeight: "600",
              color: "white",
              marginBottom: "16px",
            }}
          >
            Raw Component (No Card Wrapper)
          </h3>
          <div
            style={{
              border: "1px solid #374151",
              borderRadius: "8px",
              padding: "16px",
              height: "240px",
            }}
          >
            <TimeSeries
              series={sampleTimeSeriesData}
              yLabel="Percentage"
              showLegend={true}
            />
          </div>
        </div>
      </div>
    );
  },
};

export const TimeSeriesFillableAreas: StoryObj<typeof TimeSeries> = {
  render: (args) => {
    return (
      <div style={{ padding: "16px" }}>
        <div style={{ maxWidth: "800px", margin: "0 auto" }}>
          <Card>
            <CardHeader>
              <CardTitle>Fillable Areas Demo</CardTitle>
            </CardHeader>
            <CardContent>
              <div style={{ height: "240px" }}>
                <TimeSeries {...args} />
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    );
  },
  args: {
    series: [
      {
        name: "Filled Series",
        data: [
          { t: Date.now() - 60000 * 5, y: 20 },
          { t: Date.now() - 60000 * 4, y: 35 },
          { t: Date.now() - 60000 * 3, y: 25 },
          { t: Date.now() - 60000 * 2, y: 45 },
          { t: Date.now() - 60000 * 1, y: 40 },
          { t: Date.now(), y: 50 },
        ],
        fill: true,
      },
      {
        name: "Line Only",
        data: [
          { t: Date.now() - 60000 * 5, y: 60 },
          { t: Date.now() - 60000 * 4, y: 75 },
          { t: Date.now() - 60000 * 3, y: 65 },
          { t: Date.now() - 60000 * 2, y: 85 },
          { t: Date.now() - 60000 * 1, y: 80 },
          { t: Date.now(), y: 90 },
        ],
        fill: false,
      },
    ],
    yLabel: "Value",
    showLegend: true,
  },
};
