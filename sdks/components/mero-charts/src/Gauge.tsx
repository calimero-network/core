import React, { useMemo } from "react";

export type Thresholds = { warning: number; danger: number };

export type GaugeProps = {
  value: number;
  min?: number;
  max?: number;
  title?: string;
  label?: string;
  decimals?: number;
  thresholds?: Thresholds;
  size?: "sm" | "md" | "lg" | "xl";
  className?: string;
};

const defaultThresholds: Thresholds = { warning: 60, danger: 80 };

function clamp(n: number, lo: number, hi: number) {
  return Math.max(lo, Math.min(hi, n));
}

function polarToCartesian(cx: number, cy: number, r: number, angle: number) {
  return { x: cx + r * Math.cos(angle), y: cy + r * Math.sin(angle) };
}

function arcPath(
  cx: number,
  cy: number,
  r: number,
  start: number,
  end: number,
) {
  const s = polarToCartesian(cx, cy, r, start);
  const e = polarToCartesian(cx, cy, r, end);
  const largeArc = Math.abs(end - start) <= Math.PI ? 0 : 1;
  return `M ${s.x} ${s.y} A ${r} ${r} 0 ${largeArc} 1 ${e.x} ${e.y}`;
}

export function Gauge({
  value,
  min = 0,
  max = 100,
  title,
  label,
  decimals = 1,
  thresholds = defaultThresholds,
  size = "md",
  className = "",
}: GaugeProps) {
  // Calculate percentage based on min/max range
  const pct = useMemo(() => {
    const normalizedValue = ((value - min) / Math.max(1e-9, max - min)) * 100;
    return clamp(normalizedValue, 0, 100);
  }, [value, min, max]);

  const cx = 120,
    cy = 140; // Center positioned to keep arcs within bounds
  const outer = 100;
  const inner = 70;

  // Arc angles: start from 0.95π to 2.05π
  const startAngle = 0.95 * Math.PI;
  const endAngle = 2.05 * Math.PI;
  const totalAngle = Math.abs(endAngle - startAngle); // ~1.1π radians (~198 degrees)

  // Use a radius that makes arcs start from bottom edge but stay within bounds
  const arcRadius = 100; // Radius to make arcs start from bottom edge

  // Calculate the end angle for the value arc (this changes the LENGTH, not the angle range)
  const valueEndAngle = startAngle + (pct / 100) * (endAngle - startAngle);

  const valueText = value.toFixed(decimals);

  // Determine color based on value
  const getValueColor = (val: number) => {
    if (val <= thresholds.warning) return "#A5FF11"; // brand-600 (good)
    if (val <= thresholds.danger) return "#f59e0b"; // warning
    return "#ef4444"; // danger
  };

  const valueColor = getValueColor(pct);

  return (
    <div
      style={{
        width: "100%",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
      }}
    >
      <svg viewBox="0 0 240 160" style={{ width: "100%" }}>
        {/* background arc - single grey background */}
        <path
          d={arcPath(cx, cy, arcRadius, startAngle, endAngle)}
          stroke="#6B7280"
          strokeOpacity={0.3}
          strokeWidth={18}
          fill="none"
          strokeLinecap="butt"
        />
        {/* value arc - LENGTH changes based on value, angle range stays fixed */}
        <path
          d={arcPath(cx, cy, arcRadius, startAngle, valueEndAngle)}
          stroke={valueColor}
          strokeWidth={18}
          fill="none"
          strokeLinecap="butt"
        />
        {/* external threshold arc - shows good, warning and danger zones */}
        <path
          d={arcPath(
            cx,
            cy,
            arcRadius + 15,
            startAngle,
            startAngle + (thresholds.warning / 100) * (endAngle - startAngle),
          )}
          stroke="#A5FF11"
          strokeOpacity={0.3}
          strokeWidth={4}
          fill="none"
          strokeLinecap="butt"
        />
        <path
          d={arcPath(
            cx,
            cy,
            arcRadius + 15,
            startAngle + (thresholds.warning / 100) * (endAngle - startAngle),
            startAngle + (thresholds.danger / 100) * (endAngle - startAngle),
          )}
          stroke="#f59e0b"
          strokeOpacity={0.3}
          strokeWidth={4}
          fill="none"
          strokeLinecap="butt"
        />
        <path
          d={arcPath(
            cx,
            cy,
            arcRadius + 15,
            startAngle + (thresholds.danger / 100) * (endAngle - startAngle),
            endAngle,
          )}
          stroke="#ef4444"
          strokeOpacity={0.3}
          strokeWidth={4}
          fill="none"
          strokeLinecap="butt"
        />
        {/* inner cutout to create donut */}
        <circle cx={cx} cy={cy} r={inner} fill="#1F2A15" />{" "}
        {/* brand background in center */}
        {/* center text - color changes based on value */}
        <text
          x={cx}
          y={cy - 8}
          textAnchor="middle"
          dominantBaseline="middle"
          style={{ fontWeight: 600, fontSize: "28px", fill: valueColor }}
        >
          {valueText}
        </text>
        {label && (
          <text
            x={cx}
            y={cy + 14}
            textAnchor="middle"
            style={{ fontSize: "12px", fill: valueColor }}
          >
            {label}
          </text>
        )}
      </svg>
    </div>
  );
}
