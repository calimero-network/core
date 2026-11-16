import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const ClockX = React.forwardRef<SVGSVGElement, IconProps>(
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
        viewBox="174 0 48 48"
        role="presentation"
        aria-hidden={true}
        fill="none"
        stroke="currentColor"
        strokeWidth={actualStrokeWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
        {...props}
      >
        <path d="M180 24C180 26.3638 180.466 28.7044 181.37 30.8883C182.275 33.0722 183.601 35.0565 185.272 36.7279C186.944 38.3994 188.928 39.7252 191.112 40.6298C193.296 41.5344 195.636 42 198 42C200.364 42 202.704 41.5344 204.888 40.6298C207.072 39.7252 209.056 38.3994 210.728 36.7279C212.399 35.0565 213.725 33.0722 214.63 30.8883C215.534 28.7044 216 26.3638 216 24C216 21.6362 215.534 19.2956 214.63 17.1117C213.725 14.9278 212.399 12.9435 210.728 11.2721C209.056 9.60062 207.072 8.27475 204.888 7.37017C202.704 6.46558 200.364 6 198 6C195.636 6 193.296 6.46558 191.112 7.37017C188.928 8.27475 186.944 9.60062 185.272 11.2721C183.601 12.9435 182.275 14.9278 181.37 17.1117C180.466 19.2956 180 21.6362 180 24Z" />
        <path d="M185.4 11.4004L210.6 36.6004" />
      </svg>
    );
  },
);

ClockX.displayName = "ClockX";
export { ClockX };
