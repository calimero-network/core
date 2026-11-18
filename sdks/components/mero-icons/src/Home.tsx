import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Home = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M10 24H6L24 6L42 24H38" />
        <path d="M10 24V38C10 39.0609 10.4214 40.0783 11.1716 40.8284C11.9217 41.5786 12.9391 42 14 42H34C35.0609 42 36.0783 41.5786 36.8284 40.8284C37.5786 40.0783 38 39.0609 38 38V24" />
        <path d="M20 24H28V32H20V24Z" />
      </svg>
    );
  },
);

Home.displayName = "Home";
export { Home };
