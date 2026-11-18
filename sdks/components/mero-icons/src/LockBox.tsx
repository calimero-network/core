import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const LockBox = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M8 16V12C8 10.892 8.45 9.89002 9.176 9.16602" />
        <path d="M8 32V36C8 37.0609 8.42143 38.0783 9.17157 38.8284C9.92172 39.5786 10.9391 40 12 40H16" />
        <path d="M32 8H36C37.0609 8 38.0783 8.42143 38.8284 9.17157C39.5786 9.92172 40 10.9391 40 12V16" />
        <path d="M32 39.9999H36C37.1 39.9999 38.1 39.5559 38.82 38.8359" />
        <path d="M30 22C30.5304 22 31.0391 22.2107 31.4142 22.5858C31.7893 22.9609 32 23.4696 32 24M31.42 31.408C31.2341 31.5955 31.0129 31.7444 30.7692 31.846C30.5255 31.9476 30.2641 31.9999 30 32H18C17.4696 32 16.9609 31.7893 16.5858 31.4142C16.2107 31.0391 16 30.5304 16 30V24C16 23.4696 16.2107 22.9609 16.5858 22.5858C16.9609 22.2107 17.4696 22 18 22H22" />
        <path d="M20 21.9999V19.9999M22.364 14.3479C22.9733 14.0748 23.6412 13.9586 24.3069 14.0098C24.9726 14.0611 25.6149 14.2781 26.1752 14.6412C26.7355 15.0042 27.196 15.5018 27.5148 16.0885C27.8335 16.6751 28.0003 17.3322 28 17.9999V19.9999" />
        <path d="M6 6L42 42" />
      </svg>
    );
  },
);

LockBox.displayName = "LockBox";
export { LockBox };
