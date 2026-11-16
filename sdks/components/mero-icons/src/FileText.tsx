import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const FileText = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M23 42H12C10.9391 42 9.92172 41.5786 9.17157 40.8284C8.42143 40.0783 8 39.0609 8 38V14C8 12.9391 8.42143 11.9217 9.17157 11.1716C9.92172 10.4214 10.9391 10 12 10H36C37.0609 10 38.0783 10.4214 38.8284 11.1716C39.5786 11.9217 40 12.9391 40 14V26" />
        <path d="M32 6V14" />
        <path d="M16 6V14" />
        <path d="M8 22H40" />
        <path d="M40 42L44 38L40 34" />
        <path d="M34 34L30 38L34 42" />
      </svg>
    );
  },
);

FileText.displayName = "FileText";
export { FileText };
