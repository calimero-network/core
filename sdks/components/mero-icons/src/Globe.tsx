import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Globe = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M6 24C6 26.3638 6.46558 28.7044 7.37017 30.8883C8.27475 33.0722 9.60062 35.0565 11.2721 36.7279C12.9435 38.3994 14.9278 39.7252 17.1117 40.6298C19.2956 41.5344 21.6362 42 24 42C26.3638 42 28.7044 41.5344 30.8883 40.6298C33.0722 39.7252 35.0565 38.3994 36.7279 36.7279C38.3994 35.0565 39.7252 33.0722 40.6298 30.8883C41.5344 28.7044 42 26.3638 42 24C42 19.2261 40.1036 14.6477 36.7279 11.2721C33.3523 7.89642 28.7739 6 24 6C19.2261 6 14.6477 7.89642 11.2721 11.2721C7.89642 14.6477 6 19.2261 6 24Z" />
        <path d="M7.19995 18H40.7999" />
        <path d="M7.19995 30H40.7999" />
        <path d="M23 6C19.6306 11.3992 17.8444 17.6357 17.8444 24C17.8444 30.3643 19.6306 36.6008 23 42" />
        <path d="M25 6C28.3693 11.3992 30.1556 17.6357 30.1556 24C30.1556 30.3643 28.3693 36.6008 25 42" />
      </svg>
    );
  },
);

Globe.displayName = "Globe";
export { Globe };
