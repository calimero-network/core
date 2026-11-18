import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const LockX = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M8 16V12C8 10.9391 8.42143 9.92172 9.17157 9.17157C9.92172 8.42143 10.9391 8 12 8H16" />
        <path d="M8 32V36C8 37.0609 8.42143 38.0783 9.17157 38.8284C9.92172 39.5786 10.9391 40 12 40H16" />
        <path d="M32 8H36C37.0609 8 38.0783 8.42143 38.8284 9.17157C39.5786 9.92172 40 10.9391 40 12V16" />
        <path d="M32 40H36C37.0609 40 38.0783 39.5786 38.8284 38.8284C39.5786 38.0783 40 37.0609 40 36V32" />
        <path d="M16 24C16 23.4696 16.2107 22.9609 16.5858 22.5858C16.9609 22.2107 17.4696 22 18 22H30C30.5304 22 31.0391 22.2107 31.4142 22.5858C31.7893 22.9609 32 23.4696 32 24V30C32 30.5304 31.7893 31.0391 31.4142 31.4142C31.0391 31.7893 30.5304 32 30 32H18C17.4696 32 16.9609 31.7893 16.5858 31.4142C16.2107 31.0391 16 30.5304 16 30V24Z" />
        <path d="M20 22V18C20 16.9391 20.4214 15.9217 21.1716 15.1716C21.9217 14.4214 22.9391 14 24 14C25.0609 14 26.0783 14.4214 26.8284 15.1716C27.5786 15.9217 28 16.9391 28 18V22" />
      </svg>
    );
  },
);

LockX.displayName = "LockX";
export { LockX };
