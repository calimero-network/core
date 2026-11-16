import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Circle = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M12 40V36C12 32.8174 13.2643 29.7652 15.5147 27.5147C17.7652 25.2643 20.8174 24 24 24C27.1826 24 30.2348 25.2643 32.4853 27.5147C34.7357 29.7652 36 32.8174 36 36V40C36 40.5304 35.7893 41.0391 35.4142 41.4142C35.0391 41.7893 34.5304 42 34 42H14C13.4696 42 12.9609 41.7893 12.5858 41.4142C12.2107 41.0391 12 40.5304 12 40Z" />
        <path d="M12 8V12C12 15.1826 13.2643 18.2348 15.5147 20.4853C17.7652 22.7357 20.8174 24 24 24C27.1826 24 30.2348 22.7357 32.4853 20.4853C34.7357 18.2348 36 15.1826 36 12V8C36 7.46957 35.7893 6.96086 35.4142 6.58579C35.0391 6.21071 34.5304 6 34 6H14C13.4696 6 12.9609 6.21071 12.5858 6.58579C12.2107 6.96086 12 7.46957 12 8Z" />
      </svg>
    );
  },
);

Circle.displayName = "Circle";
export { Circle };
