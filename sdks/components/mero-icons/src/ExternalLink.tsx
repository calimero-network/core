import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const ExternalLink = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M32 6H42V16" />
        <path d="M16 6H6V16" />
        <path d="M42 6L26.928 21.072C25.0537 22.9468 24.0006 25.489 24 28.14V42" />
        <path d="M6 6L21.072 21.072C22.9463 22.9468 23.9994 25.489 24 28.14V30" />
      </svg>
    );
  },
);

ExternalLink.displayName = "ExternalLink";
export { ExternalLink };
