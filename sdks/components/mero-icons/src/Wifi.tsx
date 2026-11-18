import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Wifi = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M24 34V42" />
        <path d="M20 40L28 36" />
        <path d="M20 36L28 40" />
        <path d="M10 34V42" />
        <path d="M6 40L14 36" />
        <path d="M6 36L14 40" />
        <path d="M38 34V42" />
        <path d="M34 40L42 36" />
        <path d="M34 36L42 40" />
        <path d="M18 12C18 13.5913 18.6321 15.1174 19.7574 16.2426C20.8826 17.3679 22.4087 18 24 18C25.5913 18 27.1174 17.3679 28.2426 16.2426C29.3679 15.1174 30 13.5913 30 12C30 10.4087 29.3679 8.88258 28.2426 7.75736C27.1174 6.63214 25.5913 6 24 6C22.4087 6 20.8826 6.63214 19.7574 7.75736C18.6321 8.88258 18 10.4087 18 12Z" />
        <path d="M14 28C14 26.9391 14.4214 25.9217 15.1716 25.1716C15.9217 24.4214 16.9391 24 18 24H30C31.0609 24 32.0783 24.4214 32.8284 25.1716C33.5786 25.9217 34 26.9391 34 28" />
      </svg>
    );
  },
);

Wifi.displayName = "Wifi";
export { Wifi };
