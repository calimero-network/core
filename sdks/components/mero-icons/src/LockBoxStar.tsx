import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const LockBoxStar = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M24 26C25.5913 26 27.1174 25.3679 28.2426 24.2426C29.3679 23.1174 30 21.5913 30 20C30 18.4087 29.3679 16.8826 28.2426 15.7574C27.1174 14.6321 25.5913 14 24 14C22.4087 14 20.8826 14.6321 19.7574 15.7574C18.6321 16.8826 18 18.4087 18 20C18 21.5913 18.6321 23.1174 19.7574 24.2426C20.8826 25.3679 22.4087 26 24 26Z" />
        <path d="M12.402 37.488C12.9303 35.8904 13.949 34.5001 15.3131 33.5148C16.6772 32.5295 18.3173 31.9995 20 32H28C29.6816 31.9995 31.3207 32.5288 32.6843 33.5129C34.0479 34.4971 35.0667 35.8858 35.596 37.482" />
        <path d="M39.75 12.5392C41.15 13.3352 42.01 14.8252 42 16.4352V31.0032C42 32.6212 41.114 34.1132 39.684 34.8992L26.184 43.4392C25.5147 43.8066 24.7635 43.9993 24 43.9993C23.2365 43.9993 22.4853 43.8066 21.816 43.4392L8.316 34.8992C7.61633 34.5168 7.03225 33.9535 6.62482 33.2681C6.2174 32.5828 6.0016 31.8005 6 31.0032V16.4332C6 14.8152 6.886 13.3252 8.316 12.5392L21.816 4.57918C22.5051 4.19925 23.2791 4 24.066 4C24.8529 4 25.6269 4.19925 26.316 4.57918L39.816 12.5392H39.75Z" />
      </svg>
    );
  },
);

LockBoxStar.displayName = "LockBoxStar";
export { LockBoxStar };
