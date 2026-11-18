import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const MessageBubble = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M20 40C20 41.0609 20.4214 42.0783 21.1716 42.8284C21.9217 43.5786 22.9391 44 24 44C25.0609 44 26.0783 43.5786 26.8284 42.8284C27.5786 42.0783 28 41.0609 28 40C28 38.9391 27.5786 37.9217 26.8284 37.1716C26.0783 36.4214 25.0609 36 24 36C22.9391 36 21.9217 36.4214 21.1716 37.1716C20.4214 37.9217 20 38.9391 20 40Z" />
        <path d="M20 40H8" />
        <path d="M28 40H40" />
        <path d="M24 30L20 26H14C13.4696 26 12.9609 25.7893 12.5858 25.4142C12.2107 25.0391 12 24.5304 12 24V8C12 7.46957 12.2107 6.96086 12.5858 6.58579C12.9609 6.21071 13.4696 6 14 6H34C34.5304 6 35.0391 6.21071 35.4142 6.58579C35.7893 6.96086 36 7.46957 36 8V24C36 24.5304 35.7893 25.0391 35.4142 25.4142C35.0391 25.7893 34.5304 26 34 26H28L24 30Z" />
      </svg>
    );
  },
);

MessageBubble.displayName = "MessageBubble";
export { MessageBubble };
