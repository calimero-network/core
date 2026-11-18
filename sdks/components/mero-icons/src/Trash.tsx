import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Trash = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M8 14H40" />
        <path d="M20 22V34" />
        <path d="M28 22V34" />
        <path d="M10 14L12 38C12 39.0609 12.4214 40.0783 13.1716 40.8284C13.9217 41.5786 14.9391 42 16 42H32C33.0609 42 34.0783 41.5786 34.8284 40.8284C35.5786 40.0783 36 39.0609 36 38L38 14" />
        <path d="M18 14V8C18 7.46957 18.2107 6.96086 18.5858 6.58579C18.9609 6.21071 19.4696 6 20 6H28C28.5304 6 29.0391 6.21071 29.4142 6.58579C29.7893 6.96086 30 7.46957 30 8V14" />
      </svg>
    );
  },
);

Trash.displayName = "Trash";
export { Trash };
