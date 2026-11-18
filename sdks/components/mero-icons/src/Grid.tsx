import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Grid = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M8 8H20V20H8V8Z" />
        <path d="M28 8H40V20H28V8Z" />
        <path d="M8 28H20V40H8V28Z" />
        <path d="M28 34C28 35.5913 28.6321 37.1174 29.7574 38.2426C30.8826 39.3679 32.4087 40 34 40C35.5913 40 37.1174 39.3679 38.2426 38.2426C39.3679 37.1174 40 35.5913 40 34C40 32.4087 39.3679 30.8826 38.2426 29.7574C37.1174 28.6321 35.5913 28 34 28C32.4087 28 30.8826 28.6321 29.7574 29.7574C28.6321 30.8826 28 32.4087 28 34Z" />
      </svg>
    );
  },
);

Grid.displayName = "Grid";
export { Grid };
