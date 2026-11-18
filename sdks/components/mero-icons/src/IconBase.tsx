import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

export const IconBase = React.forwardRef<SVGSVGElement, IconProps>(
  (
    {
      size = 24,
      strokeWidth = 1.5,
      absoluteStrokeWidth = false,
      children,
      ...props
    },
    ref,
  ) => {
    // Calculate the actual stroke width based on absoluteStrokeWidth setting
    // When absoluteStrokeWidth is false, stroke width scales with size (24px = 1.5, so scale proportionally)
    // When absoluteStrokeWidth is true, stroke width remains constant
    const actualStrokeWidth = absoluteStrokeWidth
      ? strokeWidth
      : (Number(size) / 24) * Number(strokeWidth);

    return (
      <svg
        ref={ref}
        width={size}
        height={size}
        viewBox="0 0 24 24"
        role="presentation"
        aria-hidden={true}
        fill="none"
        stroke="currentColor"
        strokeWidth={actualStrokeWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
        {...props}
      >
        {children}
      </svg>
    );
  },
);
IconBase.displayName = "IconBase";
