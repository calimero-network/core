import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const ArrowLeft = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M20 36H28" />
        <path d="M6 16C10.7739 16 15.3523 17.8964 18.7279 21.2721C22.1036 24.6477 24 29.2261 24 34V36L26.856 27.43C28.8692 21.3916 33.1987 16.4003 38.892 13.554L40 13" />
        <path d="M30 12H40V22" />
      </svg>
    );
  },
);

ArrowLeft.displayName = "ArrowLeft";
export { ArrowLeft };
