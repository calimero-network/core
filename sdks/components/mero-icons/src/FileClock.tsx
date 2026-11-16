import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const FileClock = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M21 42H12C10.9391 42 9.92172 41.5786 9.17157 40.8284C8.42143 40.0783 8 39.0609 8 38V14C8 12.9391 8.42143 11.9217 9.17157 11.1716C9.92172 10.4214 10.9391 10 12 10H36C37.0609 10 38.0783 10.4214 38.8284 11.1716C39.5786 11.9217 40 12.9391 40 14V20" />
        <path d="M32 6V14" />
        <path d="M16 6V14" />
        <path d="M8 22H28" />
        <path d="M28 36C28 38.1217 28.8429 40.1566 30.3431 41.6569C31.8434 43.1571 33.8783 44 36 44C38.1217 44 40.1566 43.1571 41.6569 41.6569C43.1571 40.1566 44 38.1217 44 36C44 33.8783 43.1571 31.8434 41.6569 30.3431C40.1566 28.8429 38.1217 28 36 28C33.8783 28 31.8434 28.8429 30.3431 30.3431C28.8429 31.8434 28 33.8783 28 36Z" />
        <path d="M36 33V36L37 37" />
      </svg>
    );
  },
);

FileClock.displayName = "FileClock";
export { FileClock };
