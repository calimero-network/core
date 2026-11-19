import React from "react";

export type ToastVariant = "default" | "success" | "warning" | "error" | "info";

export type Toast = {
  id: string;
  title?: string;
  description?: string;
  variant?: ToastVariant;
  durationMs?: number;
};

type ToastContextValue = {
  toasts: Toast[];
  show: (toast: Omit<Toast, "id">) => string;
  dismiss: (id: string) => void;
  clear: () => void;
};

const ToastContext = React.createContext<ToastContextValue | null>(null);

export function useToast() {
  const ctx = React.useContext(ToastContext);
  if (!ctx) throw new Error("useToast must be used within <ToastProvider>");
  return ctx;
}

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const [toasts, setToasts] = React.useState<Toast[]>([]);

  const show = React.useCallback((toast: Omit<Toast, "id">) => {
    const id = Math.random().toString(36).slice(2);
    const next: Toast = { id, durationMs: 3000, variant: "default", ...toast };
    setToasts((prev) => [...prev, next]);
    if (next.durationMs && next.durationMs > 0) {
      window.setTimeout(() => {
        setToasts((prev) => prev.filter((t) => t.id !== id));
      }, next.durationMs);
    }
    return id;
  }, []);

  const dismiss = React.useCallback((id: string) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const clear = React.useCallback(() => setToasts([]), []);

  const value: ToastContextValue = React.useMemo(
    () => ({ toasts, show, dismiss, clear }),
    [toasts, show, dismiss, clear],
  );

  return (
    <ToastContext.Provider value={value}>
      {children}
      <ToastViewport toasts={toasts} onDismiss={dismiss} />
    </ToastContext.Provider>
  );
}

function ToastViewport({
  toasts,
  onDismiss,
}: {
  toasts: Toast[];
  onDismiss: (id: string) => void;
}) {
  return (
    <div
      style={{
        position: "fixed",
        right: "16px",
        bottom: "16px",
        display: "flex",
        flexDirection: "column",
        gap: "8px",
        zIndex: 10050,
      }}
    >
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} onDismiss={onDismiss} />
      ))}
    </div>
  );
}

function ToastItem({
  toast,
  onDismiss,
}: {
  toast: Toast;
  onDismiss: (id: string) => void;
}) {
  const palette = getVariantPalette(toast.variant || "default");
  return (
    <div
      role="status"
      style={{
        minWidth: "280px",
        maxWidth: "360px",
        padding: "12px 14px",
        borderRadius: "var(--radius-md)",
        border: `1px solid ${palette.border}`,
        background: palette.bg,
        color: palette.text,
        boxShadow: "0 8px 24px rgba(0,0,0,0.35)",
        fontFamily: "var(--font-body)",
      }}
    >
      <div style={{ display: "flex", gap: "10px", alignItems: "flex-start" }}>
        <div style={{ lineHeight: 0, color: palette.icon }}>
          <svg
            width="18"
            height="18"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
          >
            <circle cx="12" cy="12" r="10" />
            <line x1="12" y1="8" x2="12" y2="12" />
            <line x1="12" y1="16" x2="12.01" y2="16" />
          </svg>
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          {toast.title && (
            <div
              style={{
                color: "#FFFFFF",
                fontWeight: 600,
                fontSize: "14px",
                marginBottom: toast.description ? "2px" : 0,
              }}
            >
              {toast.title}
            </div>
          )}
          {toast.description && (
            <div
              style={{ fontSize: "13px", color: "var(--color-neutral-300)" }}
            >
              {toast.description}
            </div>
          )}
        </div>
        <button
          aria-label="Dismiss"
          onClick={() => onDismiss(toast.id)}
          style={{
            background: "transparent",
            border: "none",
            color: "var(--color-neutral-400)",
            cursor: "pointer",
            lineHeight: 0,
          }}
        >
          <svg
            width="16"
            height="16"
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
    </div>
  );
}

function getVariantPalette(variant: ToastVariant) {
  switch (variant) {
    case "success":
      return {
        border: "var(--color-semantic-success)",
        bg: "rgba(22, 163, 74, 0.1)",
        text: "var(--color-neutral-300)",
        icon: "var(--color-semantic-success)",
      };
    case "warning":
      return {
        border: "var(--color-semantic-warning)",
        bg: "rgba(245, 158, 11, 0.1)",
        text: "var(--color-neutral-300)",
        icon: "var(--color-semantic-warning)",
      };
    case "error":
      return {
        border: "var(--color-semantic-error)",
        bg: "rgba(239, 68, 68, 0.1)",
        text: "var(--color-neutral-300)",
        icon: "var(--color-semantic-error)",
      };
    case "info":
      return {
        border: "var(--color-semantic-info)",
        bg: "rgba(59, 130, 246, 0.1)",
        text: "var(--color-neutral-300)",
        icon: "var(--color-semantic-info)",
      };
    default:
      return {
        border: "var(--color-neutral-600)",
        bg: "var(--color-background-secondary)",
        text: "var(--color-neutral-300)",
        icon: "var(--color-neutral-300)",
      };
  }
}
