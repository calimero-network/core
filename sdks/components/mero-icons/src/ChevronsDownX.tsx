import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const ChevronsDownX = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M10.352 10.3535C10.126 10.8555 10 11.4135 10 11.9995V17.9995C10 21.3135 8.21 23.9995 6 23.9995C8.21 23.9995 10 26.6855 10 29.9995V35.9995C10 37.0604 10.4214 38.0778 11.1716 38.8279C11.9217 39.5781 12.9391 39.9995 14 39.9995" />
        <path d="M34 8C35.0609 8 36.0783 8.42143 36.8284 9.17157C37.5786 9.92172 38 10.9391 38 12V18C38 21.314 39.79 24 42 24C39.79 24 38 26.686 38 30M37.648 37.642C37.3317 38.3447 36.8191 38.941 36.172 39.3593C35.5248 39.7776 34.7706 40.0001 34 40" />
        <path d="M6 6L42 42" />
      </svg>
    );
  },
);

ChevronsDownX.displayName = "ChevronsDownX";
export { ChevronsDownX };
