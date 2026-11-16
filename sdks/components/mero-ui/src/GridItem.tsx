import React from "react";

export type GridItemProps = React.PropsWithChildren<{
  /** Column span at base breakpoint */
  colSpan?: number;
  /** Column start line (1-based). If provided, overrides flow positioning */
  colStart?: number;
  /** Row span at base breakpoint */
  rowSpan?: number;
  /** Row start line (1-based) */
  rowStart?: number;
  /** Optional className passthrough */
  className?: string;
  /** Custom inline styles */
  style?: React.CSSProperties;
}>;

export function GridItem({
  children,
  colSpan,
  colStart,
  rowSpan,
  rowStart,
  className = "",
  style,
}: GridItemProps) {
  const computedStyle: React.CSSProperties = {
    gridColumn: (() => {
      if (colStart && colSpan) return `${colStart} / span ${colSpan}`;
      if (colStart) return `${colStart} / auto`;
      if (colSpan) return `span ${colSpan} / auto`;
      return undefined;
    })(),
    gridRow: (() => {
      if (rowStart && rowSpan) return `${rowStart} / span ${rowSpan}`;
      if (rowStart) return `${rowStart} / auto`;
      if (rowSpan) return `span ${rowSpan} / auto`;
      return undefined;
    })(),
    ...style,
  };

  return (
    <div className={className} style={computedStyle}>
      {children}
    </div>
  );
}

export default GridItem;
