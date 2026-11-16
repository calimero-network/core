import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Moon = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M24 6.00037C24.264 6.00037 24.526 6.00037 24.786 6.00037C22.2167 8.38779 20.5652 11.6001 20.1186 15.0788C19.672 18.5576 20.4585 22.0828 22.3415 25.0418C24.2244 28.0008 27.0847 30.2065 30.4252 31.2753C33.7657 32.3442 37.3751 32.2087 40.626 30.8924C39.3755 33.9014 37.3316 36.5144 34.7124 38.4529C32.0932 40.3913 28.9969 41.5825 25.7538 41.8992C22.5107 42.2159 19.2425 41.6463 16.2978 40.2512C13.353 38.8561 10.8422 36.6878 9.03317 33.9776C7.22412 31.2674 6.18469 28.1169 6.02576 24.8623C5.86683 21.6077 6.59435 18.371 8.13073 15.4974C9.66711 12.6238 11.9547 10.2212 14.7495 8.54584C17.5443 6.87046 20.7415 5.98516 24 5.98438V6.00037Z" />
      </svg>
    );
  },
);

Moon.displayName = "Moon";
export { Moon };
