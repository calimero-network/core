import React from "react";
import { Box, BoxProps } from "./Box";

export interface FlexProps extends Omit<BoxProps, "display"> {
  direction?: "row" | "column" | "row-reverse" | "column-reverse";
  wrap?: "nowrap" | "wrap" | "wrap-reverse";
  align?: "flex-start" | "flex-end" | "center" | "baseline" | "stretch";
  justify?:
    | "flex-start"
    | "flex-end"
    | "center"
    | "space-between"
    | "space-around"
    | "space-evenly";
  gap?: "none" | "xs" | "sm" | "md" | "lg" | "xl";
  grow?: number;
  shrink?: number;
  basis?: number | string;
}

export const Flex: React.FC<FlexProps> = ({
  direction = "row",
  wrap = "nowrap",
  align = "stretch",
  justify = "flex-start",
  gap = "none",
  grow,
  shrink,
  basis,
  children,
  className = "",
  style = {},
  ...props
}) => {
  const flexStyle = {
    display: "flex",
    flexDirection: direction,
    flexWrap: wrap,
    alignItems: align,
    justifyContent: justify,
    flexGrow: grow,
    flexShrink: shrink,
    flexBasis: basis,
    ...style,
  };

  return (
    <Box {...props} className={className} style={flexStyle} gap={gap}>
      {children}
    </Box>
  );
};
