import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const LockBoxRight = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M20 10C20 11.0609 20.4214 12.0783 21.1716 12.8284C21.9217 13.5786 22.9391 14 24 14C25.0609 14 26.0783 13.5786 26.8284 12.8284C27.5786 12.0783 28 11.0609 28 10C28 8.93913 27.5786 7.92172 26.8284 7.17157C26.0783 6.42143 25.0609 6 24 6C22.9391 6 21.9217 6.42143 21.1716 7.17157C20.4214 7.92172 20 8.93913 20 10Z" />
        <path d="M6 38C6 39.0609 6.42143 40.0783 7.17157 40.8284C7.92172 41.5786 8.93913 42 10 42C11.0609 42 12.0783 41.5786 12.8284 40.8284C13.5786 40.0783 14 39.0609 14 38C14 36.9391 13.5786 35.9217 12.8284 35.1716C12.0783 34.4214 11.0609 34 10 34C8.93913 34 7.92172 34.4214 7.17157 35.1716C6.42143 35.9217 6 36.9391 6 38Z" />
        <path d="M34 38C34 39.0609 34.4214 40.0783 35.1716 40.8284C35.9217 41.5786 36.9391 42 38 42C39.0609 42 40.0783 41.5786 40.8284 40.8284C41.5786 40.0783 42 39.0609 42 38C42 36.9391 41.5786 35.9217 40.8284 35.1716C40.0783 34.4214 39.0609 34 38 34C36.9391 34 35.9217 34.4214 35.1716 35.1716C34.4214 35.9217 34 36.9391 34 38Z" />
        <path d="M18 28C18 29.5913 18.6321 31.1174 19.7574 32.2426C20.8826 33.3679 22.4087 34 24 34C25.5913 34 27.1174 33.3679 28.2426 32.2426C29.3679 31.1174 30 29.5913 30 28C30 26.4087 29.3679 24.8826 28.2426 23.7574C27.1174 22.6321 25.5913 22 24 22C22.4087 22 20.8826 22.6321 19.7574 23.7574C18.6321 24.8826 18 26.4087 18 28Z" />
        <path d="M24 14V22" />
        <path d="M13.4 35.5996L19 31.5996" />
        <path d="M34.6 35.5996L29 31.5996" />
      </svg>
    );
  },
);

LockBoxRight.displayName = "LockBoxRight";
export { LockBoxRight };
