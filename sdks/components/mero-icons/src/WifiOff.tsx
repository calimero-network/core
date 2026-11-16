import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const WifiOff = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M34 16C35.576 18 36 20 36 22V24" />
        <path d="M18 22C18 18.844 20.686 16 24 16C27.314 16 30 18.844 30 22V26" />
        <path d="M24 22V26" />
        <path d="M12 24.0001V21.2061C11.988 17.2081 14.272 13.5081 17.986 11.5061C19.8341 10.519 21.8966 10.0017 23.9918 10C26.087 9.99826 28.1504 10.5121 30 11.4961" />
        <path d="M24 34V42" />
        <path d="M20 40L28 36" />
        <path d="M20 36L28 40" />
        <path d="M10 34V42" />
        <path d="M6 40L14 36" />
        <path d="M6 36L14 40" />
        <path d="M38 34V42" />
        <path d="M34 40L42 36" />
        <path d="M34 36L42 40" />
      </svg>
    );
  },
);

WifiOff.displayName = "WifiOff";
export { WifiOff };
