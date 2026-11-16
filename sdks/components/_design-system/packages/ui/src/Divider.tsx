import React from "react";
import { tokens } from "@calimero-network/mero-tokens";

export interface DividerProps {
  orientation?: "horizontal" | "vertical";
  variant?: "solid" | "dashed" | "dotted";
  color?: "default" | "muted" | "subtle";
  thickness?: "thin" | "medium" | "thick";
  spacing?: "none" | "sm" | "md" | "lg";
  label?: string;
  labelPosition?: "left" | "center" | "right";
  className?: string;
  style?: React.CSSProperties;
}

export const Divider: React.FC<DividerProps> = ({
  orientation = "horizontal",
  variant = "solid",
  color = "default",
  thickness = "thin",
  spacing = "md",
  label,
  labelPosition = "center",
  className = "",
  style = {},
}) => {
  const thicknessMap = {
    thin: 1,
    medium: 2,
    thick: 4,
  };

  const spacingMap = {
    none: 0,
    sm: 8,
    md: 16,
    lg: 24,
  };

  const colorMap = {
    default: tokens.color.neutral[600].value,
    muted: tokens.color.neutral[400].value,
    subtle: tokens.color.neutral[300].value,
  };

  const variantStyles = {
    solid: "solid",
    dashed: "dashed",
    dotted: "dotted",
  };

  const dividerStyle = {
    border: "none",
    borderTop:
      orientation === "horizontal"
        ? `${thicknessMap[thickness]}px ${variantStyles[variant]} ${colorMap[color]}`
        : "none",
    borderLeft:
      orientation === "vertical"
        ? `${thicknessMap[thickness]}px ${variantStyles[variant]} ${colorMap[color]}`
        : "none",
    margin:
      orientation === "horizontal"
        ? `${spacingMap[spacing]}px 0`
        : `0 ${spacingMap[spacing]}px`,
    ...style,
  };

  const containerStyle = {
    display: "flex",
    alignItems: "center",
    width: orientation === "horizontal" ? "100%" : "auto",
    height: orientation === "vertical" ? "100%" : "auto",
  };

  const lineStyle = {
    flex: 1,
    height: orientation === "horizontal" ? thicknessMap[thickness] : "100%",
    width: orientation === "vertical" ? thicknessMap[thickness] : "100%",
    backgroundColor: colorMap[color],
    border: "none",
  };

  const labelStyle = {
    padding: orientation === "horizontal" ? "0 16px" : "16px 0",
    fontSize: "14px",
    color: tokens.color.neutral[300].value,
    backgroundColor: tokens.color.background.primary.value,
    whiteSpace: "nowrap" as const,
  };

  if (label) {
    return (
      <div className={className} style={{ ...containerStyle, ...style }}>
        {labelPosition === "left" && <div style={labelStyle}>{label}</div>}
        {labelPosition === "center" && <div style={lineStyle} />}
        <div style={labelStyle}>{label}</div>
        {labelPosition === "center" && <div style={lineStyle} />}
        {labelPosition === "right" && <div style={lineStyle} />}
      </div>
    );
  }

  return <hr className={className} style={dividerStyle} />;
};
