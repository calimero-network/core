import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Cloud = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M13.314 35.9998C8.17 35.9998 4 31.9858 4 27.0338C4 22.0838 8.17 18.0698 13.314 18.0698C14.1 14.5458 16.902 11.6698 20.664 10.5238C24.424 9.37977 28.576 10.1378 31.552 12.5238C34.528 14.9038 35.876 18.5378 35.092 22.0618H37.072C40.898 22.0618 44 25.1818 44 29.0338C44 32.8878 40.898 36.0078 37.07 36.0078H13.314" />
      </svg>
    );
  },
);

Cloud.displayName = "Cloud";
export { Cloud };
