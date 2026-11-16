import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Package = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M44 18L24 10L4 18L24 26L44 18ZM44 18V30" />
        <path d="M12 21.1992V31.9992C12 33.5905 13.2643 35.1166 15.5147 36.2419C17.7652 37.3671 20.8174 37.9992 24 37.9992C27.1826 37.9992 30.2348 37.3671 32.4853 36.2419C34.7357 35.1166 36 33.5905 36 31.9992V21.1992" />
      </svg>
    );
  },
);

Package.displayName = "Package";
export { Package };
