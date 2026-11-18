import React from "react";
import { Box, BoxProps } from "./Box";

export interface SpacerProps extends Omit<BoxProps, "children"> {
  size?: "xs" | "sm" | "md" | "lg" | "xl";
  axis?: "horizontal" | "vertical" | "both";
}

export const Spacer: React.FC<SpacerProps> = ({
  size = "md",
  axis = "vertical",
  className = "",
  style = {},
  ...props
}) => {
  const spacingMap = {
    xs: 4,
    sm: 8,
    md: 16,
    lg: 24,
    xl: 32,
  };

  const spacerStyle = {
    flex: "none",
    ...(axis === "horizontal" || axis === "both"
      ? { width: spacingMap[size] }
      : {}),
    ...(axis === "vertical" || axis === "both"
      ? { height: spacingMap[size] }
      : {}),
    ...style,
  };

  return <Box {...props} className={className} style={spacerStyle} />;
};
