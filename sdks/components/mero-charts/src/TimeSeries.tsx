// @ts-nocheck - React 19 compatibility issue with recharts
import React, { useMemo } from "react";
import {
  ComposedChart,
  Line,
  XAxis,
  YAxis,
  Tooltip,
  ResponsiveContainer,
  Legend,
  Area,
} from "recharts";

export type Point = { t: number | string; y: number };

export type Series = {
  name: string;
  data: Point[];
  fill?: boolean; // Whether to fill the area under the line
};

// Calculation functions for time series data
export function calculateMean(series: Series): number {
  if (!series.data.length) return 0;
  const sum = series.data.reduce((acc, point) => acc + point.y, 0);
  return sum / series.data.length;
}

export function calculateLast(series: Series): number {
  if (!series.data.length) return 0;
  return series.data[series.data.length - 1].y;
}

export function calculateMax(series: Series): number {
  if (!series.data.length) return 0;
  return Math.max(...series.data.map((point) => point.y));
}

function formatTime(ts: number | string) {
  const d = typeof ts === "number" ? new Date(ts) : new Date(ts);
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

// Auto-generate colors if not provided
const generateColors = (count: number): string[] => {
  const baseColors = [
    "#A5FF11", // Calimero green
    "#fbbf24", // Amber
    "#f59e0b", // Orange
    "#3b82f6", // Blue
    "#8b5cf6", // Purple
    "#ec4899", // Pink
    "#06b6d4", // Cyan
    "#10b981", // Emerald
    "#f97316", // Orange
    "#ef4444", // Red
  ];

  return Array.from(
    { length: count },
    (_, i) => baseColors[i % baseColors.length],
  );
};

export function TimeSeries({
  series,
  yLabel,
  colors,
  showLegend = true,
}: {
  series: Series[];
  yLabel?: string;
  colors?: string[];
  showLegend?: boolean;
}) {
  const flat = series.flatMap((s) => s.data);
  const domainY = useMemo(() => {
    if (!flat.length) return [0, 1];
    const min = Math.min(...flat.map((p) => p.y));
    const max = Math.max(...flat.map((p) => p.y));
    const pad = (max - min) * 0.1 || 1;
    return [Math.max(0, min - pad), max + pad];
  }, [flat]);

  const rows = useMemo(() => {
    const length = Math.max(...series.map((s) => s.data.length));
    const ts = series[0]?.data.map((p) => p.t) ?? [];
    return Array.from({ length }, (_, i) => {
      const row: any = { t: ts[i] ?? i };
      for (const s of series) row[s.name] = s.data[i]?.y ?? null;
      return row;
    });
  }, [series]);

  // Auto-generate colors if not provided
  const finalColors = colors || generateColors(series.length);

  return (
    <div style={{ width: "100%", height: "100%" }}>
      <ResponsiveContainer width="100%" height="100%">
        <ComposedChart
          data={rows}
          margin={{ left: 12, right: 12, top: 5, bottom: 5 }}
        >
          <XAxis dataKey="t" tickFormatter={formatTime} minTickGap={32} />
          <YAxis
            domain={domainY as [number, number]}
            tickFormatter={(v) => `${v}`}
          />
          <Tooltip
            labelFormatter={(l) => `Time: ${formatTime(l as any)}`}
            formatter={(v) => [v as number, yLabel ?? "value"]}
          />
          {showLegend && <Legend />}
          {series.map((s, index) => {
            const color = finalColors[index];
            if (s.fill) {
              return (
                <Area
                  key={s.name}
                  type="monotone"
                  dataKey={s.name}
                  stroke={color}
                  fill={color}
                  fillOpacity={0.1}
                  strokeWidth={2}
                />
              );
            } else {
              return (
                <Line
                  key={s.name}
                  type="monotone"
                  dataKey={s.name}
                  dot={false}
                  strokeWidth={2}
                  stroke={color}
                  fill="none"
                />
              );
            }
          })}
        </ComposedChart>
      </ResponsiveContainer>
    </div>
  );
}
