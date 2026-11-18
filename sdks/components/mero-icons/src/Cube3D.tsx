import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Cube3D = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M12 38C10.9391 38 9.92172 37.5786 9.17157 36.8284C8.42143 36.0783 8 35.0609 8 34V26L6 24L8 22V14C8 12.9391 8.42143 11.9217 9.17157 11.1716C9.92172 10.4214 10.9391 10 12 10" />
        <path d="M24 23.751L30 20.377" />
        <path d="M24 23.75V30.5" />
        <path d="M24 23.751L18 20.377" />
        <path d="M24 23.75L30 27.126" />
        <path d="M24 17V23.75" />
        <path d="M24 23.75L18 27.126" />
        <path d="M36 38C37.0609 38 38.0783 37.5786 38.8284 36.8284C39.5786 36.0783 40 35.0609 40 34V26L42 24L40 22V14C40 12.9391 39.5786 11.9217 38.8284 11.1716C38.0783 10.4214 37.0609 10 36 10" />
      </svg>
    );
  },
);

Cube3D.displayName = "Cube3D";
export { Cube3D };
