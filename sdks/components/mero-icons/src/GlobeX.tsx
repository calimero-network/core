import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const GlobeX = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M40.966 30.0114C42.2576 26.3671 42.3415 22.4037 41.2052 18.708C40.0689 15.0123 37.7727 11.7807 34.6567 9.49159C31.5407 7.20252 27.7704 5.97764 23.9039 5.9983C20.0375 6.01896 16.2805 7.28406 13.1891 9.6063C10.0977 11.9285 7.83625 15.1845 6.73948 18.8921C5.64272 22.5998 5.76896 26.5621 7.09952 30.1924C8.43008 33.8227 10.8942 36.9281 14.1272 39.0488C17.3602 41.1695 21.1901 42.1929 25.05 41.9674" />
        <path d="M7.19995 18H40.7999" />
        <path d="M7.19995 30H40.7999" />
        <path d="M23 6C19.6306 11.3992 17.8444 17.6357 17.8444 24C17.8444 30.3643 19.6306 36.6008 23 42" />
        <path d="M25 6C29.4703 13.1473 31.1155 21.7046 29.614 30" />
        <path d="M32 38H44" />
      </svg>
    );
  },
);

GlobeX.displayName = "GlobeX";
export { GlobeX };
