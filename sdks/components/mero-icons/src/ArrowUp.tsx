import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const ArrowUp = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M16 14L24 6L32 14" />
        <path d="M24 6V16.788C24.0002 19.0064 23.4526 21.1904 22.4059 23.1464C21.3592 25.1023 19.8458 26.7695 18 28C16.1542 29.2305 14.6408 30.8977 13.5941 32.8536C12.5474 34.8096 11.9998 36.9936 12 39.212V42" />
        <path d="M24 6V16.788C23.9998 19.0064 24.5474 21.1904 25.5941 23.1464C26.6408 25.1023 28.1542 26.7695 30 28C31.8458 29.2305 33.3592 30.8977 34.4059 32.8536C35.4526 34.8096 36.0002 36.9936 36 39.212V42" />
      </svg>
    );
  },
);

ArrowUp.displayName = "ArrowUp";
export { ArrowUp };
