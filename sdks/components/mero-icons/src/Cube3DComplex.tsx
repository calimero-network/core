import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Cube3DComplex = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M42 32.015V15.979C41.9988 15.2812 41.8132 14.596 41.4619 13.993C41.1107 13.39 40.6064 12.8904 40 12.545L26 4.52904C25.3909 4.18107 24.7015 3.99805 24 3.99805C23.2985 3.99805 22.6091 4.18107 22 4.52904L8 12.545C6.762 13.255 6 14.565 6 15.981V32.017C6 33.435 6.762 34.743 8 35.451L22 43.467C22.6091 43.815 23.2985 43.998 24 43.998C24.7015 43.998 25.3909 43.815 26 43.467L40 35.451C41.238 34.741 42 33.431 42 32.015Z" />
        <path d="M24 44V24" />
        <path d="M24 23.9999L41.46 13.9199" />
        <path d="M6.54001 13.9199L24 23.9999" />
      </svg>
    );
  },
);

Cube3DComplex.displayName = "Cube3DComplex";
export { Cube3DComplex };
