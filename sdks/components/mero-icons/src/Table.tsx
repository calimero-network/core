import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Table = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M8 10C8 9.46957 8.21071 8.96086 8.58579 8.58579C8.96086 8.21071 9.46957 8 10 8H38C38.5304 8 39.0391 8.21071 39.4142 8.58579C39.7893 8.96086 40 9.46957 40 10V38C40 38.5304 39.7893 39.0391 39.4142 39.4142C39.0391 39.7893 38.5304 40 38 40H10C9.46957 40 8.96086 39.7893 8.58579 39.4142C8.21071 39.0391 8 38.5304 8 38V10Z" />
        <path d="M8 16H40" />
        <path d="M16 8V16" />
        <path d="M19 29L22 32L28 26" />
      </svg>
    );
  },
);

Table.displayName = "Table";
export { Table };
