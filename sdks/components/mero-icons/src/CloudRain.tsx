import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const CloudRain = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M13.314 31.9998C8.17 31.9998 4 27.9858 4 23.0338C4 18.0838 8.17 14.0698 13.314 14.0698C14.1 10.5458 16.902 7.66977 20.664 6.52377C24.424 5.37977 28.576 6.13777 31.552 8.52377C34.528 10.9038 35.876 14.5378 35.092 18.0618H37.072C40.898 18.0618 44 21.1818 44 25.0338C44 28.8878 40.898 32.0078 37.07 32.0078H13.314" />
        <path d="M24 32V42" />
        <path d="M32 32V40C32 40.5304 32.2107 41.0391 32.5858 41.4142C32.9609 41.7893 33.4696 42 34 42H42" />
        <path d="M16 32V40C16 40.5304 15.7893 41.0391 15.4142 41.4142C15.0391 41.7893 14.5304 42 14 42H6" />
      </svg>
    );
  },
);

CloudRain.displayName = "CloudRain";
export { CloudRain };
