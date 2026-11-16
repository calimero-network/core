import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Refresh = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M40 22.0007C39.5107 18.4814 37.878 15.2205 35.3533 12.7204C32.8286 10.2202 29.5519 8.61949 26.028 8.16467C22.881 7.75515 19.683 8.27957 16.8315 9.67275C13.9801 11.0659 11.601 13.2664 9.98999 16.0007" />
        <path d="M8 26C8.46269 29.3302 9.95008 32.434 12.2559 34.8809C14.5618 37.3278 17.5719 38.9967 20.8688 39.6561C24.1657 40.3154 27.5861 39.9327 30.6557 38.5608C33.7253 37.189 36.292 34.8961 38 32" />
        <path d="M36 32C36 32.5304 36.2107 33.0391 36.5858 33.4142C36.9609 33.7893 37.4696 34 38 34C38.5304 34 39.0391 33.7893 39.4142 33.4142C39.7893 33.0391 40 32.5304 40 32C40 31.4696 39.7893 30.9609 39.4142 30.5858C39.0391 30.2107 38.5304 30 38 30C37.4696 30 36.9609 30.2107 36.5858 30.5858C36.2107 30.9609 36 31.4696 36 32Z" />
        <path d="M8 16C8 16.5304 8.21071 17.0391 8.58579 17.4142C8.96086 17.7893 9.46957 18 10 18C10.5304 18 11.0391 17.7893 11.4142 17.4142C11.7893 17.0391 12 16.5304 12 16C12 15.4696 11.7893 14.9609 11.4142 14.5858C11.0391 14.2107 10.5304 14 10 14C9.46957 14 8.96086 14.2107 8.58579 14.5858C8.21071 14.9609 8 15.4696 8 16Z" />
        <path d="M18 24C18 25.5913 18.6321 27.1174 19.7574 28.2426C20.8826 29.3679 22.4087 30 24 30C25.5913 30 27.1174 29.3679 28.2426 28.2426C29.3679 27.1174 30 25.5913 30 24C30 22.4087 29.3679 20.8826 28.2426 19.7574C27.1174 18.6321 25.5913 18 24 18C22.4087 18 20.8826 18.6321 19.7574 19.7574C18.6321 20.8826 18 22.4087 18 24Z" />
      </svg>
    );
  },
);

Refresh.displayName = "Refresh";
export { Refresh };
