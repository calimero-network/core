import React from "react";

export type GridProps = React.PropsWithChildren<{
  /** Number of columns at the base breakpoint */
  columns?: number;
  /** Gap between grid items, e.g., 8, 12, 16 (px) */
  gap?: number;
  /** Max width of the grid container, e.g., 1200 (px) */
  maxWidth?: number | string;
  /** Horizontal alignment of content */
  justify?: React.CSSProperties["justifyContent"];
  /** Vertical alignment of content */
  align?: React.CSSProperties["alignItems"];
  /** Custom inline styles */
  style?: React.CSSProperties;
  /** Optional className passthrough */
  className?: string;
}>;

export function Grid({
  children,
  columns = 12,
  gap = 12,
  maxWidth = "100%",
  justify,
  align,
  style,
  className = "",
}: GridProps) {
  const gridTemplateColumns = `repeat(${columns}, minmax(0, 1fr))`;

  return (
    <div
      className={className}
      style={{
        display: "grid",
        gridTemplateColumns,
        gap,
        maxWidth,
        justifyContent: justify,
        alignItems: align,
        width: "100%",
        ...style,
      }}
    >
      {children}
    </div>
  );
}

export default Grid;
