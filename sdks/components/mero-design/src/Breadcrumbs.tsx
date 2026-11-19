import React from "react";

export type BreadcrumbItem = {
  label: string;
  href?: string;
  onClick?: (
    event: React.MouseEvent<HTMLAnchorElement | HTMLSpanElement>,
  ) => void;
};

export type BreadcrumbsProps = {
  items: BreadcrumbItem[];
  separator?: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
};

export function Breadcrumbs({
  items,
  separator = "/",
  className = "",
  style,
}: BreadcrumbsProps) {
  return (
    <nav
      aria-label="Breadcrumb"
      className={className}
      style={{ fontFamily: "var(--font-body)", ...style }}
    >
      <ol
        style={{
          listStyle: "none",
          display: "flex",
          alignItems: "center",
          padding: 0,
          margin: 0,
          gap: "8px",
          color: "var(--color-neutral-400)",
        }}
      >
        {items.map((item, index) => {
          const isLast = index === items.length - 1;
          return (
            <li
              key={`${item.label}-${index}`}
              style={{
                display: "inline-flex",
                alignItems: "center",
                gap: "8px",
              }}
            >
              {item.href ? (
                <a
                  href={item.href}
                  onClick={item.onClick}
                  style={{
                    color: isLast ? "#FFFFFF" : "var(--color-neutral-300)",
                    textDecoration: "none",
                    fontWeight: isLast ? 600 : 500,
                    fontSize: "13px",
                  }}
                >
                  {item.label}
                </a>
              ) : (
                <span
                  onClick={item.onClick}
                  style={{
                    color: isLast ? "#FFFFFF" : "var(--color-neutral-300)",
                    cursor: item.onClick ? "pointer" : "default",
                    fontWeight: isLast ? 600 : 500,
                    fontSize: "13px",
                  }}
                >
                  {item.label}
                </span>
              )}
              {!isLast && (
                <span
                  aria-hidden="true"
                  style={{ color: "var(--color-neutral-600)" }}
                >
                  {separator}
                </span>
              )}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}
