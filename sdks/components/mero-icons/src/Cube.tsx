import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Cube = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M40 25V34.5C40.0131 34.7222 39.9755 34.9445 39.89 35.15C39.7565 35.4717 39.5086 35.7325 39.194 35.882L28.27 41.042C26.9348 41.6725 25.4766 41.9995 24 41.9995C22.5234 41.9995 21.0652 41.6725 19.73 41.042L8.80601 35.882C8.54821 35.7598 8.33354 35.5623 8.19035 35.3156C8.04716 35.0688 7.98219 34.7844 8.00401 34.5V25" />
        <path d="M8.86201 24.4324L20.13 19.7684C22.6083 18.7439 25.3917 18.7439 27.87 19.7684L39.138 24.4324C39.3876 24.5344 39.602 24.7071 39.7548 24.9293C39.9076 25.1514 39.9922 25.4134 39.9982 25.683C40.0041 25.9526 39.9312 26.2181 39.7883 26.4468C39.6454 26.6755 39.4389 26.8575 39.194 26.9704L28.27 32.0564C26.9324 32.6783 25.4751 33.0005 24 33.0005C22.5249 33.0005 21.0676 32.6783 19.73 32.0564L8.80601 26.9704C8.55979 26.8584 8.35185 26.6766 8.20795 26.4476C8.06405 26.2186 7.99053 25.9523 7.99649 25.6819C8.00245 25.4115 8.08764 25.1488 8.24149 24.9263C8.39534 24.7038 8.61109 24.5334 8.86201 24.4324Z" />
        <path d="M24 14V26" />
      </svg>
    );
  },
);

Cube.displayName = "Cube";
export { Cube };
