import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const LockSearch = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M23 42H14C12.9391 42 11.9217 41.5786 11.1716 40.8284C10.4214 40.0783 10 39.0609 10 38V26C10 24.9391 10.4214 23.9217 11.1716 23.1716C11.9217 22.4214 12.9391 22 14 22H34" />
        <path d="M16 22V14C16 11.8783 16.8429 9.84344 18.3431 8.34315C19.8434 6.84285 21.8783 6 24 6C26.1217 6 28.1566 6.84285 29.6569 8.34315C31.1571 9.84344 32 11.8783 32 14V22" />
        <path d="M30 36C30 37.5913 30.6321 39.1174 31.7574 40.2426C32.8826 41.3679 34.4087 42 36 42C37.5913 42 39.1174 41.3679 40.2426 40.2426C41.3679 39.1174 42 37.5913 42 36C42 34.4087 41.3679 32.8826 40.2426 31.7574C39.1174 30.6321 37.5913 30 36 30C34.4087 30 32.8826 30.6321 31.7574 31.7574C30.6321 32.8826 30 34.4087 30 36Z" />
        <path d="M40.4 40.4004L44 44.0004" />
      </svg>
    );
  },
);

LockSearch.displayName = "LockSearch";
export { LockSearch };
