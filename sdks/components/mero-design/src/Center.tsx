import React from "react";
import { Box, BoxProps } from "./Box";

export interface CenterProps
  extends Omit<BoxProps, "display" | "alignItems" | "justifyContent"> {
  inline?: boolean;
}

export const Center: React.FC<CenterProps> = ({
  inline = false,
  children,
  className = "",
  style = {},
  ...props
}) => {
  const centerStyle = {
    display: inline ? "inline-flex" : "flex",
    alignItems: "center",
    justifyContent: "center",
    ...style,
  };

  return (
    <Box {...props} className={className} style={centerStyle}>
      {children}
    </Box>
  );
};
