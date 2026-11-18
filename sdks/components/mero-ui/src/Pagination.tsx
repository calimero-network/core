import React from "react";
import { Button } from "./Button";

export type PaginationProps = {
  currentPage: number;
  totalPages: number;
  onPageChange: (page: number) => void;
  showFirstLast?: boolean;
  showPrevNext?: boolean;
  maxVisiblePages?: number;
  className?: string;
  style?: React.CSSProperties;
  disabled?: boolean;
};

export function Pagination({
  currentPage,
  totalPages,
  onPageChange,
  showFirstLast = true,
  showPrevNext = true,
  maxVisiblePages = 5,
  className = "",
  style,
  disabled = false,
}: PaginationProps) {
  const getVisiblePages = () => {
    if (totalPages <= maxVisiblePages) {
      return Array.from({ length: totalPages }, (_, i) => i + 1);
    }

    const half = Math.floor(maxVisiblePages / 2);
    let start = Math.max(1, currentPage - half);
    let end = Math.min(totalPages, start + maxVisiblePages - 1);

    if (end - start + 1 < maxVisiblePages) {
      start = Math.max(1, end - maxVisiblePages + 1);
    }

    return Array.from({ length: end - start + 1 }, (_, i) => start + i);
  };

  const visiblePages = getVisiblePages();
  const showFirstEllipsis = visiblePages[0] > 2;
  const showLastEllipsis =
    visiblePages[visiblePages.length - 1] < totalPages - 1;

  const handlePageClick = (page: number) => {
    if (disabled || page === currentPage) return;
    onPageChange(page);
  };

  const handlePrevious = () => {
    if (disabled || currentPage <= 1) return;
    onPageChange(currentPage - 1);
  };

  const handleNext = () => {
    if (disabled || currentPage >= totalPages) return;
    onPageChange(currentPage + 1);
  };

  const handleFirst = () => {
    if (disabled || currentPage <= 1) return;
    onPageChange(1);
  };

  const handleLast = () => {
    if (disabled || currentPage >= totalPages) return;
    onPageChange(totalPages);
  };

  if (totalPages <= 1) return null;

  return (
    <nav
      role="navigation"
      aria-label="Pagination"
      className={className}
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        gap: "4px",
        fontFamily: "var(--font-body)",
        ...style,
      }}
    >
      {showFirstLast && (
        <Button
          variant="secondary"
          onClick={handleFirst}
          disabled={disabled || currentPage <= 1}
          style={{ minWidth: "40px", height: "40px", padding: "0 8px" }}
          aria-label="Go to first page"
          title="Go to first page"
        >
          <svg
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            aria-hidden="true"
          >
            <polyline points="11,17 6,12 11,7" />
            <polyline points="18,17 13,12 18,7" />
          </svg>
        </Button>
      )}

      {showPrevNext && (
        <Button
          variant="secondary"
          onClick={handlePrevious}
          disabled={disabled || currentPage <= 1}
          style={{ minWidth: "40px", height: "40px", padding: "0 8px" }}
          aria-label="Go to previous page"
          title="Go to previous page"
        >
          <svg
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            aria-hidden="true"
          >
            <polyline points="15,18 9,12 15,6" />
          </svg>
        </Button>
      )}

      {showFirstEllipsis && (
        <>
          <Button
            variant="secondary"
            onClick={() => handlePageClick(1)}
            disabled={disabled}
            style={{ minWidth: "40px", height: "40px" }}
            aria-label="Go to page 1"
          >
            1
          </Button>
          <span style={{ color: "var(--color-neutral-400)", padding: "0 4px" }}>
            ...
          </span>
        </>
      )}

      {visiblePages.map((page) => (
        <Button
          key={page}
          variant={page === currentPage ? "primary" : "secondary"}
          onClick={() => handlePageClick(page)}
          disabled={disabled}
          style={{ minWidth: "40px", height: "40px" }}
          aria-label={
            page === currentPage
              ? `Current page, page ${page}`
              : `Go to page ${page}`
          }
          aria-current={page === currentPage ? "page" : undefined}
        >
          {page}
        </Button>
      ))}

      {showLastEllipsis && (
        <>
          <span style={{ color: "var(--color-neutral-400)", padding: "0 4px" }}>
            ...
          </span>
          <Button
            variant="secondary"
            onClick={() => handlePageClick(totalPages)}
            disabled={disabled}
            style={{ minWidth: "40px", height: "40px" }}
            aria-label={`Go to page ${totalPages}`}
          >
            {totalPages}
          </Button>
        </>
      )}

      {showPrevNext && (
        <Button
          variant="secondary"
          onClick={handleNext}
          disabled={disabled || currentPage >= totalPages}
          style={{ minWidth: "40px", height: "40px", padding: "0 8px" }}
          aria-label="Go to next page"
          title="Go to next page"
        >
          <svg
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            aria-hidden="true"
          >
            <polyline points="9,18 15,12 9,6" />
          </svg>
        </Button>
      )}

      {showFirstLast && (
        <Button
          variant="secondary"
          onClick={handleLast}
          disabled={disabled || currentPage >= totalPages}
          style={{ minWidth: "40px", height: "40px", padding: "0 8px" }}
          aria-label="Go to last page"
          title="Go to last page"
        >
          <svg
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            aria-hidden="true"
          >
            <polyline points="13,17 18,12 13,7" />
            <polyline points="6,17 11,12 6,7" />
          </svg>
        </Button>
      )}
    </nav>
  );
}
