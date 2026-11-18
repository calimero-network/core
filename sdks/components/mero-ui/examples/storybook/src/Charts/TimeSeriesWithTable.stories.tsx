import type { Meta, StoryObj } from "@storybook/react-vite";
import { TimeSeriesWithTable } from "@calimero-network/mero-charts";
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
} from "@calimero-network/mero-charts";

const meta: Meta<typeof TimeSeriesWithTable> = {
  title: "Charts/Time Series with Table",
  component: TimeSeriesWithTable,
  parameters: {
    layout: "centered",
  },
  tags: ["autodocs"],
};

export default meta;

// Sample data for time series with table
const sampleTimeSeriesData = [
  {
    name: "10.0.99.165:8429 - /api/v1/query",
    data: [
      { t: Date.now() - 60000 * 5, y: 11.2 },
      { t: Date.now() - 60000 * 4, y: 11.8 },
      { t: Date.now() - 60000 * 3, y: 11.5 },
      { t: Date.now() - 60000 * 2, y: 12.0 },
      { t: Date.now() - 60000 * 1, y: 11.9 },
      { t: Date.now(), y: 12.1 },
    ],
  },
  {
    name: "10.0.99.165:8429 - /api/v1/write",
    fill: true,
    data: [
      { t: Date.now() - 60000 * 5, y: 2.6 },
      { t: Date.now() - 60000 * 4, y: 2.7 },
      { t: Date.now() - 60000 * 3, y: 2.65 },
      { t: Date.now() - 60000 * 2, y: 2.8 },
      { t: Date.now() - 60000 * 1, y: 2.72 },
      { t: Date.now(), y: 2.75 },
    ],
  },
  {
    name: "10.0.99.165:8429 - /api/v1/query_range",
    data: [
      { t: Date.now() - 60000 * 5, y: 0.4 },
      { t: Date.now() - 60000 * 4, y: 0.42 },
      { t: Date.now() - 60000 * 3, y: 0.41 },
      { t: Date.now() - 60000 * 2, y: 0.45 },
      { t: Date.now() - 60000 * 1, y: 0.43 },
      { t: Date.now(), y: 0.467 },
    ],
  },
];

export const TimeSeriesWithTableExample: StoryObj<typeof TimeSeriesWithTable> =
  {
    render: (args) => {
      return (
        <div style={{ padding: "16px" }}>
          <div style={{ maxWidth: "1200px", margin: "0 auto" }}>
            <Card>
              <CardHeader>
                <CardTitle>API Performance Metrics</CardTitle>
              </CardHeader>
              <CardContent>
                <TimeSeriesWithTable {...args} />
              </CardContent>
            </Card>
          </div>
        </div>
      );
    },
    args: {
      timeSeriesData: sampleTimeSeriesData,
      timeSeriesYLabel: "req/s",
      layout: "side-by-side",
      showTimeSeriesLegend: true,
    },
    argTypes: {
      timeSeriesYLabel: {
        control: "text",
        description: "Y-axis label for time series",
      },
      layout: {
        control: "select",
        options: ["side-by-side", "table-below"],
        description: "Layout arrangement of chart and table",
      },
      showTimeSeriesLegend: {
        control: "boolean",
        description: "Show/hide the time series legend",
      },
    },
  };

export const TimeSeriesWithTableWithColors: StoryObj<
  typeof TimeSeriesWithTable
> = {
  render: (args) => {
    return (
      <div style={{ padding: "16px" }}>
        <div style={{ maxWidth: "1200px", margin: "0 auto" }}>
          <Card>
            <CardHeader>
              <CardTitle>API Performance Metrics</CardTitle>
            </CardHeader>
            <CardContent>
              <TimeSeriesWithTable {...args} />
            </CardContent>
          </Card>
        </div>
      </div>
    );
  },
  args: {
    timeSeriesData: sampleTimeSeriesData,
    timeSeriesYLabel: "req/s",
    colors: ["#fbbf24", "#f59e0b", "#3b82f6"],
    layout: "table-below",
    showTimeSeriesLegend: false,
  },
};

export const TimeSeriesWithTableNoLegend: StoryObj<typeof TimeSeriesWithTable> =
  {
    render: (args) => {
      return (
        <div style={{ padding: "16px" }}>
          <div style={{ maxWidth: "1200px", margin: "0 auto" }}>
            <Card>
              <CardHeader>
                <CardTitle>API Performance Metrics (No Legend)</CardTitle>
              </CardHeader>
              <CardContent>
                <TimeSeriesWithTable {...args} />
              </CardContent>
            </Card>
          </div>
        </div>
      );
    },
    args: {
      timeSeriesData: sampleTimeSeriesData,
      timeSeriesYLabel: "req/s",
      layout: "side-by-side",
      showTimeSeriesLegend: false,
    },
  };

export const TimeSeriesWithTableAutoColors: StoryObj<
  typeof TimeSeriesWithTable
> = {
  render: (args) => {
    return (
      <div style={{ padding: "16px" }}>
        <div style={{ maxWidth: "1200px", margin: "0 auto" }}>
          <Card>
            <CardHeader>
              <CardTitle>API Performance Metrics (Auto Colors)</CardTitle>
            </CardHeader>
            <CardContent>
              <TimeSeriesWithTable {...args} />
            </CardContent>
          </Card>
        </div>
      </div>
    );
  },
  args: {
    timeSeriesData: sampleTimeSeriesData,
    timeSeriesYLabel: "req/s",
    layout: "table-below",
    showTimeSeriesLegend: true,
    // No colors specified - will use auto-generated colors
  },
};

export const TimeSeriesWithTableCustomColumns: StoryObj<
  typeof TimeSeriesWithTable
> = {
  render: (args: any) => {
    return (
      <div style={{ padding: "16px" }}>
        <div style={{ maxWidth: "1200px", margin: "0 auto" }}>
          <Card>
            <CardHeader>
              <CardTitle>API Performance Metrics (Custom Columns)</CardTitle>
            </CardHeader>
            <CardContent>
              <TimeSeriesWithTable {...args} />
            </CardContent>
          </Card>
        </div>
      </div>
    );
  },
  args: {
    timeSeriesData: sampleTimeSeriesData,
    timeSeriesYLabel: "req/s",
    colors: ["#A5FF11", "#fbbf24", "#3b82f6"],
    layout: "side-by-side",
    showTimeSeriesLegend: true,
    columns: [
      { title: "Average", function: calculateMean },
      { title: "Current", function: calculateLast },
      { title: "Peak", function: calculateMax },
      {
        title: "Min",
        function: (series: any) =>
          Math.min(...series.data.map((p: any) => p.y)),
      },
    ],
  },
};
