import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Target = React.forwardRef<SVGSVGElement, IconProps>(
  (
    { size = 24, strokeWidth = 1.5, absoluteStrokeWidth = false, ...props },
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
        <path d="M16 24C16 26.1217 16.8429 28.1566 18.3431 29.6569C19.8434 31.1571 21.8783 32 24 32C26.1217 32 28.1566 31.1571 29.6569 29.6569C31.1571 28.1566 32 26.1217 32 24C32 21.8783 31.1571 19.8434 29.6569 18.3431C28.1566 16.8429 26.1217 16 24 16C21.8783 16 19.8434 16.8429 18.3431 18.3431C16.8429 19.8434 16 21.8783 16 24Z" />
        <path d="M32 23.9997V26.9997C32 28.3258 32.5268 29.5975 33.4645 30.5352C34.4021 31.4729 35.6739 31.9997 37 31.9997C38.3261 31.9997 39.5978 31.4729 40.5355 30.5352C41.4732 29.5975 42 28.3258 42 26.9997V23.9997C42.005 20.1319 40.764 16.3654 38.4608 13.2581C36.1576 10.1508 32.9148 7.868 29.2128 6.74789C25.5107 5.62779 21.5463 5.72994 17.9069 7.03922C14.2674 8.3485 11.1465 10.7953 9.00641 14.017C6.86633 17.2388 5.82094 21.0643 6.0251 24.9267C6.22926 28.7891 7.67211 32.483 10.1399 35.4612C12.6077 38.4395 15.9692 40.5436 19.7264 41.4619C23.4836 42.3802 27.4367 42.0638 31 40.5597" />
      </svg>
    );
  },
);

Target.displayName = "Target";
export { Target };
