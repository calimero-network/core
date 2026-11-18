import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Monitor = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M6 12C6 8.68629 8.68629 6 12 6H36C39.3137 6 42 8.68629 42 12V30C42 33.3137 39.3137 36 36 36H12C8.68629 36 6 33.3137 6 30V12Z" />
        <path d="M18 42H30" />
        <path d="M24 36V42" />
      </svg>
    );
  },
);

Monitor.displayName = "Monitor";
export { Monitor };
