import React, { useEffect } from "react";

export type ModalProps = {
  open: boolean;
  onClose: () => void;
  title?: string;
  children?: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
  closeOnOverlay?: boolean;
};

export function Modal({
  open,
  onClose,
  title,
  children,
  className = "",
  style,
  closeOnOverlay = true,
}: ModalProps) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={title}
      className={className}
      style={{
        position: "fixed",
        inset: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 10000,
        ...style,
      }}
    >
      <div
        onClick={closeOnOverlay ? onClose : undefined}
        style={{
          position: "absolute",
          inset: 0,
          background: "rgba(0,0,0,0.5)",
        }}
      />
      <div
        role="document"
        style={{
          position: "relative",
          background: "var(--color-background-primary)",
          border: "1px solid var(--color-neutral-600)",
          borderRadius: "var(--radius-lg)",
          minWidth: "320px",
          maxWidth: "560px",
          width: "90vw",
          padding: "16px",
          boxShadow: "0 12px 48px rgba(0,0,0,0.45)",
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            marginBottom: "8px",
          }}
        >
          {title && (
            <div
              style={{
                color: "#FFFFFF",
                fontWeight: 700,
                fontSize: "16px",
                fontFamily: "var(--font-body)",
              }}
            >
              {title}
            </div>
          )}
          <button
            aria-label="Close"
            onClick={onClose}
            style={{
              background: "transparent",
              border: "none",
              color: "var(--color-neutral-300)",
              cursor: "pointer",
              lineHeight: 0,
            }}
          >
            <svg
              width="20"
              height="20"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
            >
              <line x1="15" y1="9" x2="9" y2="15" />
              <line x1="9" y1="9" x2="15" y2="15" />
            </svg>
          </button>
        </div>
        <div
          style={{
            color: "var(--color-neutral-300)",
            fontFamily: "var(--font-body)",
          }}
        >
          {children}
        </div>
      </div>
    </div>
  );
}
