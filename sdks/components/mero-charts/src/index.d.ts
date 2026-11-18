import React from "react";

// Card primitives
export declare function Card(
  props: React.PropsWithChildren<{ className?: string }>,
): JSX.Element;
export declare function CardHeader(
  props: React.PropsWithChildren<{ className?: string }>,
): JSX.Element;
export declare function CardTitle(
  props: React.PropsWithChildren<{ className?: string }>,
): JSX.Element;
export declare function CardContent(
  props: React.PropsWithChildren<{ className?: string }>,
): JSX.Element;

// Gauge chart
export declare type Thresholds = { good: number; warn: number; bad: number };
export declare type GaugeProps = {
  value: number;
  min?: number;
  max?: number;
  label?: string;
  decimals?: number;
  thresholds?: Thresholds;
};
export declare function Gauge(props: GaugeProps): JSX.Element;
export declare function GaugeCard(props: {
  title: string;
  value: number;
  min?: number;
  max?: number;
  thresholds?: Thresholds;
  suffix?: string;
}): JSX.Element;

// Time series chart
export declare type Point = { t: number | string; y: number };
export declare type Series = { name: string; data: Point[] };
export declare function SeriesCard(props: {
  title: string;
  series: Series[];
  yLabel?: string;
}): JSX.Element;

// Vector table
export declare type PromVectorResult = {
  status: string;
  data: {
    resultType: "vector";
    result: Array<{ metric: Record<string, string>; value: [number, string] }>;
  };
};
export declare function VectorTable(props: {
  vector: PromVectorResult;
  title?: string;
}): JSX.Element;

// Demo
export declare function ChartsDemo(): JSX.Element;
