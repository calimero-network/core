import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Box = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M39.75 12.5392C41.15 13.3352 42.01 14.8252 42 16.4352V31.0032C42 32.6212 41.114 34.1132 39.684 34.8992L26.184 43.4392C25.5147 43.8066 24.7635 43.9993 24 43.9993C23.2365 43.9993 22.4853 43.8066 21.816 43.4392L8.316 34.8992C7.61633 34.5168 7.03225 33.9535 6.62482 33.2681C6.2174 32.5828 6.0016 31.8005 6 31.0032V16.4332C6 14.8152 6.886 13.3252 8.316 12.5392L21.816 4.57918C22.5051 4.19925 23.2791 4 24.066 4C24.8529 4 25.6269 4.19925 26.316 4.57918L39.816 12.5392H39.75Z" />
        <path d="M24 16V24" />
        <path d="M24 32H24.02" />
      </svg>
    );
  },
);

Box.displayName = "Box";
export { Box };
