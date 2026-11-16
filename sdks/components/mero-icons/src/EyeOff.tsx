import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const EyeOff = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M21.17 21.1738C20.42 21.9241 19.9987 22.9416 19.9989 24.0025C19.9991 25.0634 20.4207 26.0808 21.171 26.8308C21.9213 27.5809 22.9388 28.0021 23.9997 28.0019C25.0606 28.0017 26.078 27.5801 26.828 26.8298" />
        <path d="M33.362 33.3461C30.5565 35.1014 27.3093 36.0219 24 36.0001C16.8 36.0001 10.8 32.0001 6 24.0001C8.544 19.7601 11.424 16.6441 14.64 14.6521M20.36 12.3601C21.5581 12.1175 22.7776 11.9969 24 12.0001C31.2 12.0001 37.2 16.0001 42 24.0001C40.668 26.2201 39.242 28.1341 37.724 29.7401" />
        <path d="M6 6L42 42" />
      </svg>
    );
  },
);

EyeOff.displayName = "EyeOff";
export { EyeOff };
