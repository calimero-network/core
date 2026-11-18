import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const ChevronsDown = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M14 8C12.9391 8 11.9217 8.42143 11.1716 9.17157C10.4214 9.92172 10 10.9391 10 12V18C10 19.5913 9.57857 21.1174 8.82843 22.2426C8.07828 23.3679 7.06087 24 6 24C7.06087 24 8.07828 24.6321 8.82843 25.7574C9.57857 26.8826 10 28.4087 10 30V36C10 37.0609 10.4214 38.0783 11.1716 38.8284C11.9217 39.5786 12.9391 40 14 40" />
        <path d="M34 8C35.0609 8 36.0783 8.42143 36.8284 9.17157C37.5786 9.92172 38 10.9391 38 12V18C38 19.5913 38.4214 21.1174 39.1716 22.2426C39.9217 23.3679 40.9391 24 42 24C40.9391 24 39.9217 24.6321 39.1716 25.7574C38.4214 26.8826 38 28.4087 38 30V36C38 37.0609 37.5786 38.0783 36.8284 38.8284C36.0783 39.5786 35.0609 40 34 40" />
      </svg>
    );
  },
);

ChevronsDown.displayName = "ChevronsDown";
export { ChevronsDown };
