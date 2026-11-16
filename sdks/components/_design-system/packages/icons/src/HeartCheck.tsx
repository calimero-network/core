import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const HeartCheck = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M24 8C24 8.53043 24.2107 9.03914 24.5858 9.41421C24.9609 9.78929 25.4696 10 26 10C26.5304 10 27.0391 9.78929 27.4142 9.41421C27.7893 9.03914 28 8.53043 28 8C28 7.46957 27.7893 6.96086 27.4142 6.58579C27.0391 6.21071 26.5304 6 26 6C25.4696 6 24.9609 6.21071 24.5858 6.58579C24.2107 6.96086 24 7.46957 24 8Z" />
        <path d="M14 42L20 34" />
        <path d="M32 42L28 34L22 28L24 16" />
        <path d="M12 24L16 18L24 16L30 22L36 24" />
      </svg>
    );
  },
);

HeartCheck.displayName = "HeartCheck";
export { HeartCheck };
