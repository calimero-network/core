import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Egg = React.forwardRef<SVGSVGElement, IconProps>(
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
        viewBox="0 0 32 32"
        role="presentation"
        aria-hidden={true}
        fill="none"
        stroke="currentColor"
        strokeWidth={actualStrokeWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
        {...props}
      >
        <path d="M1.858 1.85717C-1.266 4.98117 2.532 13.8572 10.344 21.6572C18.154 29.4672 27.018 33.2652 30.144 30.1412C33.266 27.0172 29.468 18.1412 21.656 10.3412C13.846 2.53117 4.984 -1.26683 1.858 1.85717Z" />
      </svg>
    );
  },
);

Egg.displayName = "Egg";
export { Egg };
