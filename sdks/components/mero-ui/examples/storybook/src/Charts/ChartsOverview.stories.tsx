import type { Meta, StoryObj } from "@storybook/react-vite";
import { Gauge } from "@calimero-network/mero-charts";
import { TimeSeries } from "@calimero-network/mero-charts";
import { VectorTableRaw } from "@calimero-network/mero-charts";
import { TimeSeriesWithTable } from "@calimero-network/mero-charts";
import {
  Card,
  CardHeader,
  CardTitle,
  CardContent,
} from "@calimero-network/mero-ui";

const meta: Meta = {
  title: "Charts/Overview",
  parameters: {
    layout: "fullscreen",
  },
  tags: ["autodocs"],
};

export default meta;

// Sample data
const gaugeData = {
  value: 73.4,
  min: 0,
  max: 100,
  label: "%",
  thresholds: { warning: 60, danger: 80 },
};

const timeSeriesData = [
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

const vectorTableData = [
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

const combinedTimeSeriesData = [
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
    data: [
      { t: Date.now() - 60000 * 5, y: 2.6 },
      { t: Date.now() - 60000 * 4, y: 2.7 },
      { t: Date.now() - 60000 * 3, y: 2.65 },
      { t: Date.now() - 60000 * 2, y: 2.8 },
      { t: Date.now() - 60000 * 1, y: 2.72 },
      { t: Date.now(), y: 2.75 },
    ],
  },
];

export const ChartsOverview: StoryObj = {
  render: () => {
    return (
      <div
        style={{
          padding: "24px",
          background: "var(--color-background-brand)",
          minHeight: "100vh",
        }}
      >
        <h1
          style={{
            fontSize: "32px",
            fontWeight: "bold",
            color: "white",
            marginBottom: "32px",
            textAlign: "center",
          }}
        >
          Calimero Design System - Charts Overview
        </h1>

        {/* Gauge Section */}
        <section style={{ marginBottom: "48px" }}>
          <h2
            style={{
              fontSize: "24px",
              fontWeight: "600",
              color: "white",
              marginBottom: "24px",
            }}
          >
            Gauge Charts
          </h2>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))",
              gap: "24px",
            }}
          >
            <Card>
              <CardHeader>
                <CardTitle>System Health</CardTitle>
              </CardHeader>
              <CardContent>
                <Gauge {...gaugeData} />
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle>Disk Usage</CardTitle>
              </CardHeader>
              <CardContent>
                <Gauge
                  value={45.2}
                  label="%"
                  thresholds={{ warning: 70, danger: 90 }}
                />
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle>Network Load</CardTitle>
              </CardHeader>
              <CardContent>
                <Gauge
                  value={89.7}
                  label="%"
                  thresholds={{ warning: 80, danger: 95 }}
                />
              </CardContent>
            </Card>
          </div>
        </section>

        {/* Time Series Section */}
        <section style={{ marginBottom: "48px" }}>
          <h2
            style={{
              fontSize: "24px",
              fontWeight: "600",
              color: "white",
              marginBottom: "24px",
            }}
          >
            Time Series Charts
          </h2>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fit, minmax(500px, 1fr))",
              gap: "24px",
            }}
          >
            <Card>
              <CardHeader>
                <CardTitle>System Performance</CardTitle>
              </CardHeader>
              <CardContent>
                <div style={{ height: "240px" }}>
                  <TimeSeries
                    series={timeSeriesData}
                    yLabel="Percentage"
                    showLegend={true}
                  />
                </div>
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle>Single Metric</CardTitle>
              </CardHeader>
              <CardContent>
                <div style={{ height: "240px" }}>
                  <TimeSeries
                    series={[timeSeriesData[0]]}
                    yLabel="CPU %"
                    showLegend={false}
                    colors={["#A5FF11"]}
                  />
                </div>
              </CardContent>
            </Card>
          </div>
        </section>

        {/* Vector Table Section */}
        <section style={{ marginBottom: "48px" }}>
          <h2
            style={{
              fontSize: "24px",
              fontWeight: "600",
              color: "white",
              marginBottom: "24px",
            }}
          >
            Vector Tables
          </h2>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(auto-fit, minmax(500px, 1fr))",
              gap: "24px",
            }}
          >
            <Card>
              <CardHeader>
                <CardTitle>API Performance Metrics</CardTitle>
              </CardHeader>
              <CardContent>
                <VectorTableRaw data={vectorTableData} />
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle>Custom Colors</CardTitle>
              </CardHeader>
              <CardContent>
                <VectorTableRaw
                  data={vectorTableData}
                  colors={["#A5FF11", "#fbbf24", "#3b82f6"]}
                />
              </CardContent>
            </Card>
          </div>
        </section>

        {/* TimeSeries with Table Section */}
        <section style={{ marginBottom: "48px" }}>
          <h2
            style={{
              fontSize: "24px",
              fontWeight: "600",
              color: "white",
              marginBottom: "24px",
            }}
          >
            TimeSeries with Table
          </h2>
          <div
            style={{ display: "flex", flexDirection: "column", gap: "32px" }}
          >
            <Card>
              <CardHeader>
                <CardTitle>API Performance Overview</CardTitle>
              </CardHeader>
              <CardContent>
                <TimeSeriesWithTable
                  timeSeriesData={combinedTimeSeriesData}
                  timeSeriesYLabel="req/s"
                  layout="side-by-side"
                  showTimeSeriesLegend={true}
                />
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle>Metrics with Table Below</CardTitle>
              </CardHeader>
              <CardContent>
                <TimeSeriesWithTable
                  timeSeriesData={combinedTimeSeriesData}
                  timeSeriesYLabel="req/s"
                  layout="table-below"
                  showTimeSeriesLegend={false}
                />
              </CardContent>
            </Card>
          </div>
        </section>
      </div>
    );
  },
};
