import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const WifiSimple = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M24 34V42" />
        <path d="M20 40L28 36" />
        <path d="M20 36L28 40" />
        <path d="M10 34V42" />
        <path d="M6 40L14 36" />
        <path d="M6 36L14 40" />
        <path d="M38 34V42" />
        <path d="M34 40L42 36" />
        <path d="M34 36L42 40" />
        <path d="M14 28V12C14 10.9391 14.4214 9.92172 15.1716 9.17157C15.9217 8.42143 16.9391 8 18 8H30C31.0609 8 32.0783 8.42143 32.8284 9.17157C33.5786 9.92172 34 10.9391 34 12V28" />
        <path d="M22 10H26" />
        <path d="M24 34V34.02" />
      </svg>
    );
  },
);

WifiSimple.displayName = "WifiSimple";
export { WifiSimple };
