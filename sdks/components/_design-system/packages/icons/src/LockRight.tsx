import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const LockRight = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M6 26C6 24.9391 6.42143 23.9217 7.17157 23.1716C7.92172 22.4214 8.93913 22 10 22H30C31.0609 22 32.0783 22.4214 32.8284 23.1716C33.5786 23.9217 34 24.9391 34 26V38C34 39.0609 33.5786 40.0783 32.8284 40.8284C32.0783 41.5786 31.0609 42 30 42H10C8.93913 42 7.92172 41.5786 7.17157 40.8284C6.42143 40.0783 6 39.0609 6 38V26Z" />
        <path d="M18 32C18 32.5304 18.2107 33.0391 18.5858 33.4142C18.9609 33.7893 19.4696 34 20 34C20.5304 34 21.0391 33.7893 21.4142 33.4142C21.7893 33.0391 22 32.5304 22 32C22 31.4696 21.7893 30.9609 21.4142 30.5858C21.0391 30.2107 20.5304 30 20 30C19.4696 30 18.9609 30.2107 18.5858 30.5858C18.2107 30.9609 18 31.4696 18 32Z" />
        <path d="M26 22V14C26 11.8783 26.8429 9.84344 28.3431 8.34315C29.8434 6.84285 31.8783 6 34 6C36.1217 6 38.1566 6.84285 39.6569 8.34315C41.1571 9.84344 42 11.8783 42 14V22" />
      </svg>
    );
  },
);

LockRight.displayName = "LockRight";
export { LockRight };
