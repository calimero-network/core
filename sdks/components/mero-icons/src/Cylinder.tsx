import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Cylinder = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M12 12C12 8.68629 14.6863 6 18 6H30C33.3137 6 36 8.68629 36 12V36C36 39.3137 33.3137 42 30 42H18C14.6863 42 12 39.3137 12 36V12Z" />
        <path d="M12 12C12 15.3137 14.6863 18 18 18H30C33.3137 18 36 15.3137 36 12" />
      </svg>
    );
  },
);

Cylinder.displayName = "Cylinder";
export { Cylinder };
