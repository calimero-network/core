import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Eye = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M20 24C20 25.0609 20.4214 26.0783 21.1716 26.8284C21.9217 27.5786 22.9391 28 24 28C25.0609 28 26.0783 27.5786 26.8284 26.8284C27.5786 26.0783 28 25.0609 28 24C28 22.9391 27.5786 21.9217 26.8284 21.1716C26.0783 20.4214 25.0609 20 24 20C22.9391 20 21.9217 20.4214 21.1716 21.1716C20.4214 21.9217 20 22.9391 20 24Z" />
        <path d="M42 24C37.2 32 31.2 36 24 36C16.8 36 10.8 32 6 24C10.8 16 16.8 12 24 12C31.2 12 37.2 16 42 24Z" />
      </svg>
    );
  },
);

Eye.displayName = "Eye";
export { Eye };
