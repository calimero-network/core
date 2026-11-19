import React from "react";
import { Flex, FlexProps } from "./Flex";

export interface StackProps
  extends Omit<FlexProps, "direction" | "justify" | "align"> {
  direction?: "vertical" | "horizontal";
  spacing?: "none" | "xs" | "sm" | "md" | "lg" | "xl";
  align?: "start" | "center" | "end" | "stretch";
  justify?:
    | "start"
    | "center"
    | "end"
    | "space-between"
    | "space-around"
    | "space-evenly";
  divider?: React.ReactNode;
}

export const Stack: React.FC<StackProps> = ({
  direction = "vertical",
  spacing = "md",
  align = "stretch",
  justify = "start",
  divider,
  children,
  className = "",
  style = {},
  ...props
}) => {
  const flexDirection = direction === "vertical" ? "column" : "row";

  const alignMap = {
    start: "flex-start",
    center: "center",
    end: "flex-end",
    stretch: "stretch",
  } as const;

  const justifyMap = {
    start: "flex-start",
    center: "center",
    end: "flex-end",
    "space-between": "space-between",
    "space-around": "space-around",
    "space-evenly": "space-evenly",
  } as const;

  const childrenArray = React.Children.toArray(children);
  const hasDivider = divider && childrenArray.length > 1;

  const renderChildren = () => {
    if (!hasDivider) {
      return children;
    }

    return childrenArray.map((child, index) => (
      <React.Fragment key={index}>
        {child}
        {index < childrenArray.length - 1 && (
          <div style={{ alignSelf: "center" }}>{divider}</div>
        )}
      </React.Fragment>
    ));
  };

  return (
    <Flex
      {...props}
      direction={flexDirection}
      align={alignMap[align]}
      justify={justifyMap[justify]}
      gap={spacing}
      className={className}
      style={style}
    >
      {renderChildren()}
    </Flex>
  );
};
