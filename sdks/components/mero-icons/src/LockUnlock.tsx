import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const LockUnlock = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M34 42V30M38 30V27M38 45V42M34 36H40M38 36H39M39 36C39.7956 36 40.5587 36.3161 41.1213 36.8787C41.6839 37.4413 42 38.2044 42 39C42 39.7956 41.6839 40.5587 41.1213 41.1213C40.5587 41.6839 39.7956 42 39 42H32M39 36C39.7956 36 40.5587 35.6839 41.1213 35.1213C41.6839 34.5587 42 33.7956 42 33C42 32.2044 41.6839 31.4413 41.1213 30.8787C40.5587 30.3161 39.7956 30 39 30H32" />
        <path d="M26 42H14C12.9391 42 11.9217 41.5786 11.1716 40.8284C10.4214 40.0783 10 39.0609 10 38V26C10 24.9391 10.4214 23.9217 11.1716 23.1716C11.9217 22.4214 12.9391 22 14 22H34" />
        <path d="M22 32C22 32.5304 22.2107 33.0391 22.5858 33.4142C22.9609 33.7893 23.4696 34 24 34C24.5304 34 25.0391 33.7893 25.4142 33.4142C25.7893 33.0391 26 32.5304 26 32C26 31.4696 25.7893 30.9609 25.4142 30.5858C25.0391 30.2107 24.5304 30 24 30C23.4696 30 22.9609 30.2107 22.5858 30.5858C22.2107 30.9609 22 31.4696 22 32Z" />
        <path d="M16 22V14C16 11.8783 16.8429 9.84344 18.3431 8.34315C19.8434 6.84285 21.8783 6 24 6C26.1217 6 28.1566 6.84285 29.6569 8.34315C31.1571 9.84344 32 11.8783 32 14V22" />
      </svg>
    );
  },
);

LockUnlock.displayName = "LockUnlock";
export { LockUnlock };
