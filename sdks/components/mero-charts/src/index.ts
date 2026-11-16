// Gauge chart
export { Gauge } from "./Gauge";
export type { GaugeProps, Thresholds } from "./Gauge";

// Time series chart
export {
  TimeSeries,
  calculateMean,
  calculateLast,
  calculateMax,
} from "./TimeSeries";
export type { Series, Point } from "./TimeSeries";

// Vector table
export { VectorTableRaw } from "./VectorTable";
export type { VectorRow, VectorTableProps, ColumnConfig } from "./VectorTable";

// TimeSeries with table
export { TimeSeriesWithTable } from "./CombinedChart";
export type {
  TimeSeriesWithTableProps,
  ColumnConfig as TimeSeriesWithTableColumnConfig,
} from "./CombinedChart";
