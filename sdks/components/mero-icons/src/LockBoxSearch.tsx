import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const LockBoxSearch = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M20 18C20 19.0609 20.4214 20.0783 21.1716 20.8284C21.9217 21.5786 22.9391 22 24 22C25.0609 22 26.0783 21.5786 26.8284 20.8284C27.5786 20.0783 28 19.0609 28 18C28 16.9391 27.5786 15.9217 26.8284 15.1716C26.0783 14.4214 25.0609 14 24 14C22.9391 14 21.9217 14.4214 21.1716 15.1716C20.4214 15.9217 20 16.9391 20 18Z" />
        <path d="M8 16V12C8 10.9391 8.42143 9.92172 9.17157 9.17157C9.92172 8.42143 10.9391 8 12 8H16" />
        <path d="M8 32V36C8 37.0609 8.42143 38.0783 9.17157 38.8284C9.92172 39.5786 10.9391 40 12 40H16" />
        <path d="M32 8H36C37.0609 8 38.0783 8.42143 38.8284 9.17157C39.5786 9.92172 40 10.9391 40 12V16" />
        <path d="M32 40H36C37.0609 40 38.0783 39.5786 38.8284 38.8284C39.5786 38.0783 40 37.0609 40 36V32" />
        <path d="M16 32C16 30.9391 16.4214 29.9217 17.1716 29.1716C17.9217 28.4214 18.9391 28 20 28H28C29.0609 28 30.0783 28.4214 30.8284 29.1716C31.5786 29.9217 32 30.9391 32 32" />
      </svg>
    );
  },
);

LockBoxSearch.displayName = "LockBoxSearch";
export { LockBoxSearch };
