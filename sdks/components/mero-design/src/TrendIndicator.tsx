import React from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Icon } from "./Icon";

export interface TrendIndicatorProps {
  value: number;
  direction: "up" | "down" | "neutral";
  period?: string;
  color?: "success" | "error" | "neutral" | "primary";
  size?: "sm" | "md" | "lg";
  showIcon?: boolean;
  className?: string;
  style?: React.CSSProperties;
}

export function TrendIndicator({
  value,
  direction,
  period,
  color,
  size = "md",
  showIcon = true,
  className = "",
  style = {},
}: TrendIndicatorProps) {
  // Determine color based on direction if not explicitly set
  const getColor = ():
    | "primary"
    | "secondary"
    | "muted"
    | "success"
    | "warning"
    | "error"
    | "current" => {
    if (color) {
      const colorMap = {
        success: "success" as const,
        error: "error" as const,
        neutral: "muted" as const,
        primary: "primary" as const,
      };
      return colorMap[color];
    }

    if (direction === "up") return "success";
    if (direction === "down") return "error";
    return "muted";
  };

  // Determine icon based on direction
  const getIcon = () => {
    if (direction === "up") return "arrow-up";
    if (direction === "down") return "arrow-down";
    return "minus";
  };

  const sizeStyles = {
    sm: {
      fontSize: "12px",
      iconSize: "xs" as const,
      gap: "4px",
    },
    md: {
      fontSize: "14px",
      iconSize: "sm" as const,
      gap: "6px",
    },
    lg: {
      fontSize: "16px",
      iconSize: "sm" as const,
      gap: "8px",
    },
  };

  const getTextColor = () => {
    if (color) {
      const colorMap = {
        success: tokens.color.semantic.success.value,
        error: tokens.color.semantic.error.value,
        neutral: tokens.color.neutral[400].value,
        primary: tokens.color.brand[600].value,
      };
      return colorMap[color];
    }

    if (direction === "up") return tokens.color.semantic.success.value;
    if (direction === "down") return tokens.color.semantic.error.value;
    return tokens.color.neutral[400].value;
  };

  const currentSize = sizeStyles[size];

  return (
    <div
      className={className}
      style={{
        display: "flex",
        alignItems: "center",
        gap: currentSize.gap,
        color: getTextColor(),
        fontSize: currentSize.fontSize,
        fontWeight: 500,
        ...style,
      }}
    >
      {showIcon && (
        <Icon name={getIcon()} size={currentSize.iconSize} color={getColor()} />
      )}
      <span>
        {value > 0 ? "+" : ""}
        {value}%
        {period && (
          <span
            style={{
              color: tokens.color.neutral[400].value,
              fontWeight: 400,
              marginLeft: "4px",
            }}
          >
            {period}
          </span>
        )}
      </span>
    </div>
  );
}
