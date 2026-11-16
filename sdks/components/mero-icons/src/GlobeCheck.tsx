import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const GlobeCheck = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M41.8921 25.9806C42.2882 22.4017 41.602 18.7863 39.9219 15.6015C38.2419 12.4167 35.6453 9.80904 32.4677 8.11542C29.2902 6.4218 25.6777 5.7201 22.0972 6.10102C18.5166 6.48193 15.1327 7.92794 12.3825 10.2522C9.6324 12.5764 7.64256 15.672 6.67015 19.139C5.69775 22.6059 5.78748 26.2848 6.92775 29.7002C8.06802 33.1156 10.2064 36.1106 13.0666 38.298C15.9267 40.4853 19.3772 41.7647 22.9721 41.9706" />
        <path d="M7.19995 18H40.7999" />
        <path d="M7.19995 30H35" />
        <path d="M23 6C19.6306 11.3992 17.8444 17.6357 17.8444 24C17.8444 30.3643 19.6306 36.6008 23 42" />
        <path d="M25 6C29.4633 13.1509 31.1105 21.705 29.622 30.002" />
        <path d="M30 38L34 42L42 34" />
      </svg>
    );
  },
);

GlobeCheck.displayName = "GlobeCheck";
export { GlobeCheck };
