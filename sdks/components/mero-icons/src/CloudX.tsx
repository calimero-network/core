import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const CloudX = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M24 36.0078H13.314C8.17 35.9998 4 31.9858 4 27.0338C4 22.0838 8.17 18.0698 13.314 18.0698C14.1 14.5458 16.902 11.6698 20.664 10.5238C24.424 9.37977 28.576 10.1378 31.552 12.5238C34.528 14.9038 35.876 18.5378 35.092 22.0618H37.072C38.1407 22.0614 39.1948 22.3092 40.1513 22.7857C41.1079 23.2623 41.9406 23.9545 42.584 24.8078" />
        <path d="M32 38C32 39.5913 32.6321 41.1174 33.7574 42.2426C34.8826 43.3679 36.4087 44 38 44C39.5913 44 41.1174 43.3679 42.2426 42.2426C43.3679 41.1174 44 39.5913 44 38C44 36.4087 43.3679 34.8826 42.2426 33.7574C41.1174 32.6321 39.5913 32 38 32C36.4087 32 34.8826 32.6321 33.7574 33.7574C32.6321 34.8826 32 36.4087 32 38Z" />
        <path d="M34 42L42 34" />
      </svg>
    );
  },
);

CloudX.displayName = "CloudX";
export { CloudX };
