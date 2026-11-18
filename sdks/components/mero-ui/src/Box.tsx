import React from "react";
import { tokens } from "@calimero-network/mero-tokens";

export interface BoxProps {
  children?: React.ReactNode;
  as?: keyof React.JSX.IntrinsicElements;
  padding?: "none" | "xs" | "sm" | "md" | "lg" | "xl";
  margin?: "none" | "xs" | "sm" | "md" | "lg" | "xl";
  backgroundColor?:
    | "primary"
    | "secondary"
    | "muted"
    | "subtle"
    | "transparent";
  borderRadius?: "none" | "sm" | "md" | "lg" | "full";
  border?: "none" | "thin" | "medium" | "thick";
  borderColor?:
    | "default"
    | "muted"
    | "subtle"
    | "primary"
    | "success"
    | "warning"
    | "error";
  shadow?: "none" | "sm" | "md" | "lg" | "xl";
  width?: number | string;
  height?: number | string;
  minWidth?: number | string;
  minHeight?: number | string;
  maxWidth?: number | string;
  maxHeight?: number | string;
  display?:
    | "block"
    | "inline"
    | "inline-block"
    | "flex"
    | "inline-flex"
    | "grid"
    | "none";
  flexDirection?: "row" | "column" | "row-reverse" | "column-reverse";
  alignItems?: "flex-start" | "flex-end" | "center" | "baseline" | "stretch";
  justifyContent?:
    | "flex-start"
    | "flex-end"
    | "center"
    | "space-between"
    | "space-around"
    | "space-evenly";
  gap?: "none" | "xs" | "sm" | "md" | "lg" | "xl";
  position?: "static" | "relative" | "absolute" | "fixed" | "sticky";
  top?: number | string;
  right?: number | string;
  bottom?: number | string;
  left?: number | string;
  zIndex?: number;
  overflow?: "visible" | "hidden" | "scroll" | "auto";
  className?: string;
  style?: React.CSSProperties;
  onClick?: () => void;
}

export const Box: React.FC<BoxProps> = ({
  children,
  as: Component = "div",
  padding = "none",
  margin = "none",
  backgroundColor = "transparent",
  borderRadius = "none",
  border = "none",
  borderColor = "default",
  shadow = "none",
  width,
  height,
  minWidth,
  minHeight,
  maxWidth,
  maxHeight,
  display = "block",
  flexDirection,
  alignItems,
  justifyContent,
  gap = "none",
  position = "static",
  top,
  right,
  bottom,
  left,
  zIndex,
  overflow = "visible",
  className = "",
  style = {},
  onClick,
}) => {
  const spacingMap = {
    none: 0,
    xs: 4,
    sm: 8,
    md: 16,
    lg: 24,
    xl: 32,
  };

  const backgroundColorMap = {
    primary: tokens.color.background.primary.value,
    secondary: tokens.color.background.secondary.value,
    muted: tokens.color.background.tertiary.value,
    subtle: tokens.color.background.brand.value,
    transparent: "transparent",
  };

  const borderRadiusMap = {
    none: "0",
    sm: tokens.radius.sm.value,
    md: tokens.radius.md.value,
    lg: tokens.radius.lg.value,
    full: "50%",
  };

  const borderMap = {
    none: "none",
    thin: "1px solid",
    medium: "2px solid",
    thick: "4px solid",
  };

  const borderColorMap = {
    default: tokens.color.neutral[600].value,
    muted: tokens.color.neutral[400].value,
    subtle: tokens.color.neutral[300].value,
    primary: tokens.color.brand[600].value,
    success: tokens.color.semantic.success.value,
    warning: tokens.color.semantic.warning.value,
    error: tokens.color.semantic.error.value,
  };

  const shadowMap = {
    none: "none",
    sm: "0 1px 2px 0 rgba(0, 0, 0, 0.05)",
    md: "0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06)",
    lg: "0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -2px rgba(0, 0, 0, 0.05)",
    xl: "0 20px 25px -5px rgba(0, 0, 0, 0.1), 0 10px 10px -5px rgba(0, 0, 0, 0.04)",
  };

  const boxStyle = {
    padding: spacingMap[padding],
    margin: spacingMap[margin],
    backgroundColor: backgroundColorMap[backgroundColor],
    borderRadius: borderRadiusMap[borderRadius],
    border:
      border !== "none"
        ? `${borderMap[border]} ${borderColorMap[borderColor]}`
        : "none",
    boxShadow: shadowMap[shadow],
    width,
    height,
    minWidth,
    minHeight,
    maxWidth,
    maxHeight,
    display,
    flexDirection,
    alignItems,
    justifyContent,
    gap: gap !== "none" ? spacingMap[gap] : undefined,
    position,
    top,
    right,
    bottom,
    left,
    zIndex,
    overflow,
    cursor: onClick ? "pointer" : "inherit",
    ...style,
  };

  return (
    <Component className={className} style={boxStyle} onClick={onClick}>
      {children}
    </Component>
  );
};
