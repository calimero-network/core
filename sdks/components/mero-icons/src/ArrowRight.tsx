import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const ArrowRight = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M42 34H31.206C29.5982 33.9999 28.014 33.6121 26.588 32.8694C25.1619 32.1268 23.9359 31.0513 23.014 29.734L21.986 28.266C21.0641 26.9487 19.8381 25.8732 18.412 25.1306C16.986 24.3879 15.4018 24.0001 13.794 24H6" />
        <path d="M42 14H31.21C29.6011 13.9999 28.016 14.388 26.5891 15.1314C25.1623 15.8748 23.9359 16.9514 23.014 18.27L21.994 19.73C21.0723 21.0483 19.8462 22.1248 18.4197 22.8681C16.9933 23.6115 15.4085 23.9998 13.8 24H6" />
        <path d="M36 20L42 14L36 8" />
        <path d="M36 40L42 34L36 28" />
      </svg>
    );
  },
);

ArrowRight.displayName = "ArrowRight";
export { ArrowRight };
