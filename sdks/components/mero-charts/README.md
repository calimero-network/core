# @calimero/charts

A React chart library for the Calimero design system, featuring three main chart types designed for monitoring and analytics dashboards.

## Installation

```bash
pnpm add @calimero/charts recharts
```

## Chart Types

### 1. Gauge Chart

A circular gauge component perfect for displaying metrics like CPU usage, memory usage, or any percentage-based values.

```tsx
import { Gauge, GaugeCard } from '@calimero/charts';

// Basic gauge
<Gauge value={75} label="CPU" />

// Gauge with custom thresholds
<Gauge
  value={85}
  min={0}
  max={100}
  thresholds={{ good: 60, warning: 80, max: 100 }}
  decimals={1}
/>

// Gauge in a card
<GaugeCard
  title="CPU Usage"
  value={73.4}
  suffix="%"
/>
```

#### Props

- `value` (number): The current value to display
- `min` (number, optional): Minimum value (default: 0)
- `max` (number, optional): Maximum value (default: 100)
- `label` (string, optional): Label displayed below the value
- `decimals` (number, optional): Number of decimal places (default: 1)
- `thresholds` (object, optional): Color thresholds for good/warning/max states

### 2. Time Series Chart

A line chart component for displaying time-series data using Recharts.

```tsx
import { SeriesCard, Series } from "@calimero/charts";

const data: Series[] = [
  {
    name: "Requests per second",
    data: [
      { t: Date.now() - 60000, y: 12.5 },
      { t: Date.now() - 30000, y: 15.2 },
      { t: Date.now(), y: 13.8 },
    ],
  },
];

<SeriesCard title="API Requests" series={data} yLabel="req/s" />;
```

#### Props

- `title` (string): Chart title
- `series` (Series[]): Array of data series
- `yLabel` (string, optional): Y-axis label

### 3. Vector Table

A table component for displaying Prometheus-style instant vector data.

```tsx
import { VectorTable, PromVectorResult } from "@calimero/charts";

const vector: PromVectorResult = {
  status: "success",
  data: {
    resultType: "vector",
    result: [
      {
        metric: {
          instance: "10.0.99.165:8429",
          container: "vmsingle",
          reason: "unsupported",
          path: "*",
        },
        value: [1756235265, "0"],
      },
    ],
  },
};

<VectorTable title="HTTP Request Errors" vector={vector} />;
```

#### Props

- `vector` (PromVectorResult): The vector data to display
- `title` (string, optional): Table title

## Demo Component

Use the `ChartsDemo` component to see all chart types in action:

```tsx
import { ChartsDemo } from "@calimero/charts";

<ChartsDemo />;
```

## Card Primitives

The package also exports card primitives that can be used independently:

```tsx
import { Card, CardHeader, CardTitle, CardContent } from "@calimero/charts";

<Card>
  <CardHeader>
    <CardTitle>Custom Content</CardTitle>
  </CardHeader>
  <CardContent>Your content here</CardContent>
</Card>;
```

## Styling

The charts use Calimero design tokens for consistent theming:

### Colors

- **Success**: `#16a34a` (green) - Good zone
- **Warning**: `#f59e0b` (orange) - Warning zone
- **Error**: `#ef4444` (red) - Critical zone
- **Info**: `#3b82f6` (blue) - Informational
- **Background**: `#1A1A1A` (dark)
- **Border**: `#404040` (medium gray)
- **Text**: `white` and `#8E8E8E` (light gray)

The components are built with Tailwind CSS classes and can be customized by passing additional className props.

## Dependencies

- React 18+
- Recharts 2.12.0+

## Development

```bash
# Build the package
pnpm build

# Type checking
pnpm typecheck
```
