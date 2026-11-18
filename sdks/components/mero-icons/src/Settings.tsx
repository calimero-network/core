import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Settings = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M24 12C24 13.0609 24.4214 14.0783 25.1716 14.8284C25.9217 15.5786 26.9391 16 28 16C29.0609 16 30.0783 15.5786 30.8284 14.8284C31.5786 14.0783 32 13.0609 32 12C32 10.9391 31.5786 9.92172 30.8284 9.17157C30.0783 8.42143 29.0609 8 28 8C26.9391 8 25.9217 8.42143 25.1716 9.17157C24.4214 9.92172 24 10.9391 24 12Z" />
        <path d="M8 12H24" />
        <path d="M32 12H40" />
        <path d="M12 24C12 25.0609 12.4214 26.0783 13.1716 26.8284C13.9217 27.5786 14.9391 28 16 28C17.0609 28 18.0783 27.5786 18.8284 26.8284C19.5786 26.0783 20 25.0609 20 24C20 22.9391 19.5786 21.9217 18.8284 21.1716C18.0783 20.4214 17.0609 20 16 20C14.9391 20 13.9217 20.4214 13.1716 21.1716C12.4214 21.9217 12 22.9391 12 24Z" />
        <path d="M8 24H12" />
        <path d="M20 24H40" />
        <path d="M30 36C30 37.0609 30.4214 38.0783 31.1716 38.8284C31.9217 39.5786 32.9391 40 34 40C35.0609 40 36.0783 39.5786 36.8284 38.8284C37.5786 38.0783 38 37.0609 38 36C38 34.9391 37.5786 33.9217 36.8284 33.1716C36.0783 32.4214 35.0609 32 34 32C32.9391 32 31.9217 32.4214 31.1716 33.1716C30.4214 33.9217 30 34.9391 30 36Z" />
        <path d="M8 36H30" />
        <path d="M38 36H40" />
      </svg>
    );
  },
);

Settings.displayName = "Settings";
export { Settings };
