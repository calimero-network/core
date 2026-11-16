import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const ArrowDown = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M6 38C6 39.0609 6.42143 40.0783 7.17157 40.8284C7.92172 41.5786 8.93913 42 10 42C11.0609 42 12.0783 41.5786 12.8284 40.8284C13.5786 40.0783 14 39.0609 14 38C14 36.9391 13.5786 35.9217 12.8284 35.1716C12.0783 34.4214 11.0609 34 10 34C8.93913 34 7.92172 34.4214 7.17157 35.1716C6.42143 35.9217 6 36.9391 6 38Z" />
        <path d="M14 38H20C21.0609 38 22.0783 37.5786 22.8284 36.8284C23.5786 36.0783 24 35.0609 24 34V18C24 16.9391 24.4214 15.9217 25.1716 15.1716C25.9217 14.4214 26.9391 14 28 14H42" />
        <path d="M36 8L42 14L36 20" />
      </svg>
    );
  },
);

ArrowDown.displayName = "ArrowDown";
export { ArrowDown };
