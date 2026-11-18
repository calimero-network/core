import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Heart = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M24 20V28" />
        <path d="M20 26L28 22" />
        <path d="M20 22L28 26" />
        <path d="M10 20V28" />
        <path d="M6 26L14 22" />
        <path d="M6 22L14 26" />
        <path d="M38 20V28" />
        <path d="M34 26L42 22" />
        <path d="M34 22L42 26" />
      </svg>
    );
  },
);

Heart.displayName = "Heart";
export { Heart };
