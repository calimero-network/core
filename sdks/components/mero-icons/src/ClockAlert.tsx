import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const ClockAlert = React.forwardRef<SVGSVGElement, IconProps>(
  (
    { size = 24, strokeWidth = 1.5, absoluteStrokeWidth = false, ...props },
    ref,
  ) => {
    // Calculate the actual stroke width based on absoluteStrokeWidth setting
    // When absoluteStrokeWidth is false, stroke width scales with size (24px = 1.5, so scale proportionally)
    // When absoluteStrokeWidth is true, stroke width remains constant
    const actualStrokeWidth = absoluteStrokeWidth
      ? strokeWidth
      : (Number(size) / 24) * Number(strokeWidth);

    return (
      <svg
        ref={ref}
        width={size}
        height={size}
        viewBox="87 0 48 48"
        role="presentation"
        aria-hidden={true}
        fill="none"
        stroke="currentColor"
        strokeWidth={actualStrokeWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
        {...props}
      >
        <path d="M93 24C93 26.3638 93.4656 28.7044 94.3702 30.8883C95.2748 33.0722 96.6006 35.0565 98.2721 36.7279C99.9435 38.3994 101.928 39.7252 104.112 40.6298C106.296 41.5344 108.636 42 111 42C113.364 42 115.704 41.5344 117.888 40.6298C120.072 39.7252 122.056 38.3994 123.728 36.7279C125.399 35.0565 126.725 33.0722 127.63 30.8883C128.534 28.7044 129 26.3638 129 24C129 19.2261 127.104 14.6477 123.728 11.2721C120.352 7.89642 115.774 6 111 6C106.226 6 101.648 7.89642 98.2721 11.2721C94.8964 14.6477 93 19.2261 93 24Z" />
        <path d="M111 32V32.02" />
        <path d="M111 25.9995C111.9 26.0022 112.774 25.7017 113.481 25.1464C114.189 24.5911 114.689 23.8135 114.9 22.9392C115.111 22.0648 115.022 21.1448 114.646 20.3276C114.27 19.5105 113.629 18.8439 112.828 18.4355C112.032 18.0279 111.122 17.9016 110.246 18.077C109.369 18.2524 108.578 18.7192 108 19.4015" />
      </svg>
    );
  },
);

ClockAlert.displayName = "ClockAlert";
export { ClockAlert };
