import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const LockBoxDown = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M24 30C24 31.5913 24.6321 33.1174 25.7574 34.2426C26.8826 35.3679 28.4087 36 30 36C31.5913 36 33.1174 35.3679 34.2426 34.2426C35.3679 33.1174 36 31.5913 36 30C36 28.4087 35.3679 26.8826 34.2426 25.7574C33.1174 24.6321 31.5913 24 30 24C28.4087 24 26.8826 24.6321 25.7574 25.7574C24.6321 26.8826 24 28.4087 24 30Z" />
        <path d="M26 35V44L30 41L34 44V35" />
        <path d="M20 38H10C8.93913 38 7.92172 37.5786 7.17157 36.8284C6.42143 36.0783 6 35.0609 6 34V14C6 11.8 7.8 10 10 10H38C39.0609 10 40.0783 10.4214 40.8284 11.1716C41.5786 11.9217 42 12.9391 42 14V34C41.9993 34.7015 41.8141 35.3904 41.4631 35.9977C41.112 36.605 40.6075 37.1093 40 37.46" />
        <path d="M12 18H36" />
        <path d="M12 24H18" />
        <path d="M12 30H16" />
      </svg>
    );
  },
);

LockBoxDown.displayName = "LockBoxDown";
export { LockBoxDown };
