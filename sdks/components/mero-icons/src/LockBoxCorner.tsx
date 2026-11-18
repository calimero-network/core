import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const LockBoxCorner = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M24 30H11C9.67392 30 8.40215 29.4732 7.46447 28.5355C6.52678 27.5979 6 26.3261 6 25C6 23.6739 6.52678 22.4021 7.46447 21.4645C8.40215 20.5268 9.67392 20 11 20H12" />
        <path d="M30 24V37C30 38.3261 29.4732 39.5979 28.5355 40.5355C27.5979 41.4732 26.3261 42 25 42C23.6739 42 22.4021 41.4732 21.4645 40.5355C20.5268 39.5979 20 38.3261 20 37V36" />
        <path d="M24 18H37C38.3261 18 39.5979 18.5268 40.5355 19.4645C41.4732 20.4021 42 21.6739 42 23C42 24.3261 41.4732 25.5979 40.5355 26.5355C39.5979 27.4732 38.3261 28 37 28H36" />
        <path d="M18 24V11C18 9.67392 18.5268 8.40215 19.4645 7.46447C20.4021 6.52678 21.6739 6 23 6C24.3261 6 25.5979 6.52678 26.5355 7.46447C27.4732 8.40215 28 9.67392 28 11V12" />
      </svg>
    );
  },
);

LockBoxCorner.displayName = "LockBoxCorner";
export { LockBoxCorner };
