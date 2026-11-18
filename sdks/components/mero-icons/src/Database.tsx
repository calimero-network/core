import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Database = React.forwardRef<SVGSVGElement, IconProps>(
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
        <path d="M31 26C29.1435 26 27.363 26.7375 26.0503 28.0503C24.7375 29.363 24 31.1435 24 33V35C24 36.8565 24.7375 38.637 26.0503 39.9497C27.363 41.2625 29.1435 42 31 42C32.8565 42 34.637 41.2625 35.9497 39.9497C37.2625 38.637 38 36.8565 38 35V31.4" />
        <path d="M17 26C18.8565 26 20.637 26.7375 21.9497 28.0503C23.2625 29.363 24 31.1435 24 33V35C24 36.8565 23.2625 38.637 21.9497 39.9497C20.637 41.2625 18.8565 42 17 42C15.1435 42 13.363 41.2625 12.0503 39.9497C10.7375 38.637 10 36.8565 10 35V31.4" />
        <path d="M35 32C36.8565 32 38.637 31.2625 39.9497 29.9497C41.2625 28.637 42 26.8565 42 25C42 23.1435 41.2625 21.363 39.9497 20.0503C38.637 18.7375 36.8565 18 35 18H34" />
        <path d="M38 18.6V13C38 11.1435 37.2625 9.36301 35.9497 8.05025C34.637 6.7375 32.8565 6 31 6C29.1435 6 27.363 6.7375 26.0503 8.05025C24.7375 9.36301 24 11.1435 24 13" />
        <path d="M13 32C11.1435 32 9.36301 31.2625 8.05025 29.9497C6.7375 28.637 6 26.8565 6 25C6 23.1435 6.7375 21.363 8.05025 20.0503C9.36301 18.7375 11.1435 18 13 18H14" />
        <path d="M10 18.6V13C10 11.1435 10.7375 9.36301 12.0503 8.05025C13.363 6.7375 15.1435 6 17 6C18.8565 6 20.637 6.7375 21.9497 8.05025C23.2625 9.36301 24 11.1435 24 13V33" />
      </svg>
    );
  },
);

Database.displayName = "Database";
export { Database };
