import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const BarChart = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M6 26C6 25.4696 6.21071 24.9609 6.58579 24.5858C6.96086 24.2107 7.46957 24 8 24H16C16.5304 24 17.0391 24.2107 17.4142 24.5858C17.7893 24.9609 18 25.4696 18 26V38C18 38.5304 17.7893 39.0391 17.4142 39.4142C17.0391 39.7893 16.5304 40 16 40H8C7.46957 40 6.96086 39.7893 6.58579 39.4142C6.21071 39.0391 6 38.5304 6 38V26Z" />
        <path d="M30 18C30 17.4696 30.2107 16.9609 30.5858 16.5858C30.9609 16.2107 31.4696 16 32 16H40C40.5304 16 41.0391 16.2107 41.4142 16.5858C41.7893 16.9609 42 17.4696 42 18V38C42 38.5304 41.7893 39.0391 41.4142 39.4142C41.0391 39.7893 40.5304 40 40 40H32C31.4696 40 30.9609 39.7893 30.5858 39.4142C30.2107 39.0391 30 38.5304 30 38V18Z" />
        <path d="M18 10C18 9.46957 18.2107 8.96086 18.5858 8.58579C18.9609 8.21071 19.4696 8 20 8H28C28.5304 8 29.0391 8.21071 29.4142 8.58579C29.7893 8.96086 30 9.46957 30 10V38C30 38.5304 29.7893 39.0391 29.4142 39.4142C29.0391 39.7893 28.5304 40 28 40H20C19.4696 40 18.9609 39.7893 18.5858 39.4142C18.2107 39.0391 18 38.5304 18 38V10Z" />
        <path d="M8 40H36" />
      </svg>
    );
  },
);

BarChart.displayName = "BarChart";
export { BarChart };
