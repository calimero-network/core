import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const MessageCircle = React.forwardRef<SVGSVGElement, IconProps>(
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
        viewBox="0 0 38 36"
        role="presentation"
        aria-hidden={true}
        fill="none"
        stroke="currentColor"
        strokeWidth={actualStrokeWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
        {...props}
      >
        <path d="M13 29H7C5.4087 29 3.88258 28.3679 2.75736 27.2426C1.63214 26.1174 1 24.5913 1 23V7C1 5.4087 1.63214 3.88258 2.75736 2.75736C3.88258 1.63214 5.4087 1 7 1H31C32.5913 1 34.1174 1.63214 35.2426 2.75736C36.3679 3.88258 37 5.4087 37 7V23C37 24.5913 36.3679 26.1174 35.2426 27.2426C34.1174 28.3679 32.5913 29 31 29H25L19 35L13 29Z" />
      </svg>
    );
  },
);

MessageCircle.displayName = "MessageCircle";
export { MessageCircle };
