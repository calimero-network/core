import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const LockBoxLeft = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M6 14C6 15.0609 6.42143 16.0783 7.17157 16.8284C7.92172 17.5786 8.93913 18 10 18C11.0609 18 12.0783 17.5786 12.8284 16.8284C13.5786 16.0783 14 15.0609 14 14C14 12.9391 13.5786 11.9217 12.8284 11.1716C12.0783 10.4214 11.0609 10 10 10C8.93913 10 7.92172 10.4214 7.17157 11.1716C6.42143 11.9217 6 12.9391 6 14Z" />
        <path d="M28 30C28 31.0609 28.4214 32.0783 29.1716 32.8284C29.9217 33.5786 30.9391 34 32 34C33.0609 34 34.0783 33.5786 34.8284 32.8284C35.5786 32.0783 36 31.0609 36 30C36 28.9391 35.5786 27.9217 34.8284 27.1716C34.0783 26.4214 33.0609 26 32 26C30.9391 26 29.9217 26.4214 29.1716 27.1716C28.4214 27.9217 28 28.9391 28 30Z" />
        <path d="M30 12C30 13.5913 30.6321 15.1174 31.7574 16.2426C32.8826 17.3679 34.4087 18 36 18C37.5913 18 39.1174 17.3679 40.2426 16.2426C41.3679 15.1174 42 13.5913 42 12C42 10.4087 41.3679 8.88258 40.2426 7.75736C39.1174 6.63214 37.5913 6 36 6C34.4087 6 32.8826 6.63214 31.7574 7.75736C30.6321 8.88258 30 10.4087 30 12Z" />
        <path d="M6 36C6 37.5913 6.63214 39.1174 7.75736 40.2426C8.88258 41.3679 10.4087 42 12 42C13.5913 42 15.1174 41.3679 16.2426 40.2426C17.3679 39.1174 18 37.5913 18 36C18 34.4087 17.3679 32.8826 16.2426 31.7574C15.1174 30.6321 13.5913 30 12 30C10.4087 30 8.88258 30.6321 7.75736 31.7574C6.63214 32.8826 6 34.4087 6 36Z" />
        <path d="M18 34L28 31" />
        <path d="M13 17L28.62 27.74" />
        <path d="M14 14L30 12" />
      </svg>
    );
  },
);

LockBoxLeft.displayName = "LockBoxLeft";
export { LockBoxLeft };
