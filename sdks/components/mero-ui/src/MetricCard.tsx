import React from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Card } from "./Card";
import { Icon } from "./Icon";

export interface SparklineData {
  value: number;
  timestamp: string | number;
}

export interface MetricCardProps {
  title: string;
  value: string | number;
  subtitle?: string;
  sparkline?: SparklineData[];
  trend?: {
    value: number;
    direction: "up" | "down" | "neutral";
    period?: string;
  };
  icon?: React.ReactNode;
  color?: "primary" | "success" | "warning" | "error" | "info" | "neutral";
  size?: "small" | "medium" | "large";
  loading?: boolean;
  className?: string;
  style?: React.CSSProperties;
  onClick?: () => void;
  showSparkline?: boolean;
  sparklineHeight?: number;
  sparklineColor?: string;
  threshold?: {
    value: number;
    type: "above" | "below" | "between";
    color?: string;
  };
}

export function MetricCard({
  title,
  value,
  subtitle,
  sparkline = [],
  trend,
  icon,
  color = "primary",
  size = "medium",
  loading = false,
  className = "",
  style = {},
  onClick,
  showSparkline = true,
  sparklineHeight = 40,
  sparklineColor,
  threshold,
}: MetricCardProps) {
  const colorStyles = {
    primary: {
      background: `linear-gradient(135deg, ${tokens.color.brand[600].value}20, ${tokens.color.brand[600].value}10)`,
      border: `1px solid ${tokens.color.brand[600].value}40`,
      iconColor: tokens.color.brand[600].value,
      valueColor: "#FFFFFF",
      sparklineColor: tokens.color.brand[600].value,
    },
    success: {
      background: `linear-gradient(135deg, ${tokens.color.semantic.success.value}20, ${tokens.color.semantic.success.value}10)`,
      border: `1px solid ${tokens.color.semantic.success.value}40`,
      iconColor: tokens.color.semantic.success.value,
      valueColor: "#FFFFFF",
      sparklineColor: tokens.color.semantic.success.value,
    },
    warning: {
      background: `linear-gradient(135deg, ${tokens.color.semantic.warning.value}20, ${tokens.color.semantic.warning.value}10)`,
      border: `1px solid ${tokens.color.semantic.warning.value}40`,
      iconColor: tokens.color.semantic.warning.value,
      valueColor: "#FFFFFF",
      sparklineColor: tokens.color.semantic.warning.value,
    },
    error: {
      background: `linear-gradient(135deg, ${tokens.color.semantic.error.value}20, ${tokens.color.semantic.error.value}10)`,
      border: `1px solid ${tokens.color.semantic.error.value}40`,
      iconColor: tokens.color.semantic.error.value,
      valueColor: "#FFFFFF",
      sparklineColor: tokens.color.semantic.error.value,
    },
    info: {
      background: `linear-gradient(135deg, ${tokens.color.semantic.info.value}20, ${tokens.color.semantic.info.value}10)`,
      border: `1px solid ${tokens.color.semantic.info.value}40`,
      iconColor: tokens.color.semantic.info.value,
      valueColor: "#FFFFFF",
      sparklineColor: tokens.color.semantic.info.value,
    },
    neutral: {
      background: `linear-gradient(135deg, ${tokens.color.neutral[600].value}20, ${tokens.color.neutral[600].value}10)`,
      border: `1px solid ${tokens.color.neutral[600].value}40`,
      iconColor: tokens.color.neutral[400].value,
      valueColor: "#FFFFFF",
      sparklineColor: tokens.color.neutral[400].value,
    },
  };

  const sizeStyles = {
    small: {
      padding: "16px",
      titleSize: "12px",
      valueSize: "20px",
      subtitleSize: "10px",
      iconSize: "16px",
    },
    medium: {
      padding: "20px",
      titleSize: "14px",
      valueSize: "28px",
      subtitleSize: "12px",
      iconSize: "20px",
    },
    large: {
      padding: "24px",
      titleSize: "16px",
      valueSize: "36px",
      subtitleSize: "14px",
      iconSize: "24px",
    },
  };

  const currentColorStyle = colorStyles[color];
  const currentSizeStyle = sizeStyles[size];

  // Generate sparkline SVG
  const generateSparkline = () => {
    if (!sparkline || sparkline.length < 2) return null;

    const width = 120;
    const height = sparklineHeight;
    const padding = 4;

    const values = sparkline.map((d) => d.value);
    const minValue = Math.min(...values);
    const maxValue = Math.max(...values);
    const range = maxValue - minValue || 1;

    const points = sparkline
      .map((d, index) => {
        const x =
          (index / (sparkline.length - 1)) * (width - padding * 2) + padding;
        const y =
          height -
          padding -
          ((d.value - minValue) / range) * (height - padding * 2);
        return `${x},${y}`;
      })
      .join(" ");

    const sparklineColorToUse =
      sparklineColor || currentColorStyle.sparklineColor;

    return (
      <svg width={width} height={height} style={{ marginTop: "8px" }}>
        <polyline
          points={points}
          fill="none"
          stroke={sparklineColorToUse}
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
        {sparkline.map((d, index) => {
          const x =
            (index / (sparkline.length - 1)) * (width - padding * 2) + padding;
          const y =
            height -
            padding -
            ((d.value - minValue) / range) * (height - padding * 2);
          return (
            <circle
              key={index}
              cx={x}
              cy={y}
              r="2"
              fill={sparklineColorToUse}
            />
          );
        })}
      </svg>
    );
  };

  // Check threshold status
  const getThresholdStatus = () => {
    if (!threshold) return null;

    const numericValue =
      typeof value === "string"
        ? parseFloat(value.replace(/[^0-9.-]/g, ""))
        : value;

    switch (threshold.type) {
      case "above":
        return numericValue > threshold.value;
      case "below":
        return numericValue < threshold.value;
      case "between":
        return (
          Array.isArray(threshold.value) &&
          numericValue >= threshold.value[0] &&
          numericValue <= threshold.value[1]
        );
      default:
        return null;
    }
  };

  const thresholdStatus = getThresholdStatus();
  const thresholdColor =
    threshold?.color ||
    (thresholdStatus
      ? tokens.color.semantic.success.value
      : tokens.color.semantic.error.value);

  if (loading) {
    return (
      <Card
        className={className}
        style={{
          ...currentColorStyle,
          ...currentSizeStyle,
          ...style,
          cursor: "default",
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
          }}
        >
          <div style={{ flex: 1 }}>
            <div
              style={{
                height: currentSizeStyle.titleSize,
                background: tokens.color.neutral[700].value,
                borderRadius: "4px",
                marginBottom: "8px",
                width: "60%",
              }}
            />
            <div
              style={{
                height: currentSizeStyle.valueSize,
                background: tokens.color.neutral[700].value,
                borderRadius: "4px",
                marginBottom: "4px",
                width: "40%",
              }}
            />
            <div
              style={{
                height: currentSizeStyle.subtitleSize,
                background: tokens.color.neutral[700].value,
                borderRadius: "4px",
                width: "30%",
              }}
            />
          </div>
          <div
            style={{
              width: currentSizeStyle.iconSize,
              height: currentSizeStyle.iconSize,
              background: tokens.color.neutral[700].value,
              borderRadius: "50%",
            }}
          />
        </div>
      </Card>
    );
  }

  return (
    <div
      className={className}
      style={{
        cursor: onClick ? "pointer" : "default",
        transition: "all 0.2s ease",
      }}
      onClick={onClick}
      onMouseEnter={(e: React.MouseEvent<HTMLDivElement>) => {
        if (onClick) {
          e.currentTarget.style.transform = "translateY(-2px)";
          e.currentTarget.style.boxShadow = "0 8px 25px rgba(0, 0, 0, 0.3)";
        }
      }}
      onMouseLeave={(e: React.MouseEvent<HTMLDivElement>) => {
        if (onClick) {
          e.currentTarget.style.transform = "translateY(0)";
          e.currentTarget.style.boxShadow = "none";
        }
      }}
    >
      <Card
        style={{
          ...currentColorStyle,
          ...currentSizeStyle,
          ...style,
          borderColor:
            thresholdStatus !== null
              ? thresholdColor + "40"
              : currentColorStyle.border,
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "flex-start",
            justifyContent: "space-between",
          }}
        >
          <div style={{ flex: 1, minWidth: 0 }}>
            <div
              style={{
                fontSize: currentSizeStyle.titleSize,
                color: tokens.color.neutral[300].value,
                fontWeight: 500,
                marginBottom: "8px",
                lineHeight: 1.2,
              }}
            >
              {title}
            </div>

            <div
              style={{
                fontSize: currentSizeStyle.valueSize,
                color:
                  thresholdStatus !== null
                    ? thresholdColor
                    : currentColorStyle.valueColor,
                fontWeight: 700,
                marginBottom: "4px",
                lineHeight: 1.1,
                fontFamily: "var(--font-heading)",
              }}
            >
              {value}
            </div>

            {subtitle && (
              <div
                style={{
                  fontSize: currentSizeStyle.subtitleSize,
                  color: tokens.color.neutral[400].value,
                  lineHeight: 1.2,
                }}
              >
                {subtitle}
              </div>
            )}

            {trend && (
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: "4px",
                  marginTop: "8px",
                }}
              >
                <span
                  style={{
                    fontSize: currentSizeStyle.subtitleSize,
                    color:
                      trend.direction === "up"
                        ? tokens.color.semantic.success.value
                        : trend.direction === "down"
                          ? tokens.color.semantic.error.value
                          : tokens.color.neutral[400].value,
                    fontWeight: 600,
                  }}
                >
                  {trend.direction === "up"
                    ? "↗"
                    : trend.direction === "down"
                      ? "↘"
                      : "→"}{" "}
                  {Math.abs(trend.value)}%
                </span>
                {trend.period && (
                  <span
                    style={{
                      fontSize: currentSizeStyle.subtitleSize,
                      color: tokens.color.neutral[400].value,
                    }}
                  >
                    {trend.period}
                  </span>
                )}
              </div>
            )}

            {threshold && (
              <div
                style={{
                  fontSize: currentSizeStyle.subtitleSize,
                  color: thresholdColor,
                  fontWeight: 600,
                  marginTop: "4px",
                }}
              >
                {thresholdStatus
                  ? "✓ Within threshold"
                  : "⚠ Outside threshold"}
              </div>
            )}
          </div>

          {icon && (
            <div
              style={{
                color: currentColorStyle.iconColor,
                fontSize: currentSizeStyle.iconSize,
                marginLeft: "12px",
                flexShrink: 0,
              }}
            >
              {icon}
            </div>
          )}
        </div>

        {/* Sparkline */}
        {showSparkline && sparkline && sparkline.length > 0 && (
          <div style={{ marginTop: "12px" }}>{generateSparkline()}</div>
        )}
      </Card>
    </div>
  );
}
