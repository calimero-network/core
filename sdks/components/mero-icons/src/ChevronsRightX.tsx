import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const ChevronsRightX = React.forwardRef<SVGSVGElement, IconProps>(
  (
    { size = 24, strokeWidth = 1.5, absoluteStrokeWidth = false, ...props },
    ref,
  ) => {
    const actualStrokeWidth = absoluteStrokeWidth
      ? strokeWidth
      : (Number(size) / 24) * Number(strokeWidth);

    return (
      <svg
        ref={ref}
        width={size}
        height={size}
        viewBox="0 0 48 48"
        role="presentation"
        aria-hidden={true}
        fill="none"
        stroke="currentColor"
        strokeWidth={actualStrokeWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
        {...props}
      >
        <path d="M16 8H16.02" />
        <path d="M12.906 12.9473L6 23.9993L16 39.9993" />
        <path d="M32 8L42 24L38.166 30.134" />
        <path d="M35.07 35.0879L32 39.9999" />
        <path d="M6 6L42 42" />
      </svg>
    );
  },
);

ChevronsRightX.displayName = "ChevronsRightX";
export { ChevronsRightX };
