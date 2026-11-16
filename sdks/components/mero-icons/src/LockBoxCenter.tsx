import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const LockBoxCenter = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M12 18C12 21.1826 13.2643 24.2348 15.5147 26.4853C17.7652 28.7357 20.8174 30 24 30C27.1826 30 30.2348 28.7357 32.4853 26.4853C34.7357 24.2348 36 21.1826 36 18C36 14.8174 34.7357 11.7652 32.4853 9.51472C30.2348 7.26428 27.1826 6 24 6C20.8174 6 17.7652 7.26428 15.5147 9.51472C13.2643 11.7652 12 14.8174 12 18Z" />
        <path d="M24 30L30.8 41.78L33.996 35.314L41.192 35.778L34.392 24" />
        <path d="M13.604 24L6.80396 35.78L14 35.314L17.196 41.778L23.996 30" />
      </svg>
    );
  },
);

LockBoxCenter.displayName = "LockBoxCenter";
export { LockBoxCenter };
