import React, { useMemo } from "react";
import {
  TimeSeries,
  Series,
  calculateMean,
  calculateLast,
  calculateMax,
} from "./TimeSeries";
import { VectorTableRaw, VectorRow } from "./VectorTable";

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

// Column configuration type
export type ColumnConfig = {
  title: string;
  function: (series: Series) => number;
};

export type TimeSeriesWithTableProps = {
  timeSeriesData: Series[];
  vectorTableData?: VectorRow[]; // Optional now since we can calculate from timeSeriesData
  colors?: string[];
  timeSeriesYLabel?: string;
  layout?: "side-by-side" | "table-below";
  showTimeSeriesLegend?: boolean;
  // Column configurations
  columns?: ColumnConfig[];
};

export function TimeSeriesWithTable({
  timeSeriesData,
  vectorTableData,
  colors,
  timeSeriesYLabel,
  layout = "side-by-side",
  showTimeSeriesLegend = true,
  columns = [
    { title: "Mean", function: calculateMean },
    { title: "Last *", function: calculateLast },
    { title: "Max", function: calculateMax },
  ],
}: TimeSeriesWithTableProps) {
  // Generate colors if not provided
  const finalColors = useMemo(() => {
    if (colors) {
      return colors;
    }

    return generateColors(timeSeriesData.length);
  }, [colors, timeSeriesData.length]);

  // Calculate table data from time series if not provided
  const finalTableData = useMemo(() => {
    if (vectorTableData) {
      return vectorTableData;
    }

    return timeSeriesData.map((series) => {
      const row: any = { name: series.name };
      columns.forEach((column) => {
        row[column.title.toLowerCase().replace(/\s+/g, "_")] =
          column.function(series);
      });
      return row;
    });
  }, [vectorTableData, timeSeriesData, columns]);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: layout === "side-by-side" ? "row" : "column",
        gap: "24px",
        minHeight: "240px",
      }}
    >
      <div
        style={{
          height: layout === "side-by-side" ? "240px" : "240px",
          flex: layout === "side-by-side" ? "1" : "none",
        }}
      >
        <TimeSeries
          series={timeSeriesData}
          yLabel={timeSeriesYLabel}
          colors={finalColors}
          showLegend={showTimeSeriesLegend}
        />
      </div>

      {(layout === "table-below" || layout === "side-by-side") && (
        <div
          style={{
            flex: layout === "side-by-side" ? "1" : "none",
          }}
        >
          <VectorTableRaw
            data={finalTableData}
            colors={finalColors}
            columns={columns}
          />
        </div>
      )}
    </div>
  );
}
