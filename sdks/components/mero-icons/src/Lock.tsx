import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Lock = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M10 26C10 24.9391 10.4214 23.9217 11.1716 23.1716C11.9217 22.4214 12.9391 22 14 22H34C35.0609 22 36.0783 22.4214 36.8284 23.1716C37.5786 23.9217 38 24.9391 38 26V38C38 39.0609 37.5786 40.0783 36.8284 40.8284C36.0783 41.5786 35.0609 42 34 42H14C12.9391 42 11.9217 41.5786 11.1716 40.8284C10.4214 40.0783 10 39.0609 10 38V26Z" />
        <path d="M22 32C22 32.5304 22.2107 33.0391 22.5858 33.4142C22.9609 33.7893 23.4696 34 24 34C24.5304 34 25.0391 33.7893 25.4142 33.4142C25.7893 33.0391 26 32.5304 26 32C26 31.4696 25.7893 30.9609 25.4142 30.5858C25.0391 30.2107 24.5304 30 24 30C23.4696 30 22.9609 30.2107 22.5858 30.5858C22.2107 30.9609 22 31.4696 22 32Z" />
        <path d="M16 22V14C16 11.8783 16.8429 9.84344 18.3431 8.34315C19.8434 6.84285 21.8783 6 24 6C26.1217 6 28.1566 6.84285 29.6569 8.34315C31.1571 9.84344 32 11.8783 32 14V22" />
      </svg>
    );
  },
);

Lock.displayName = "Lock";
export { Lock };
