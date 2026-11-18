import React from "react";

export type AlertVariant = "info" | "success" | "warning" | "error";

export type AlertProps = {
  variant?: AlertVariant;
  title?: string;
  description?: string;
  icon?: React.ReactNode;
  dismissible?: boolean;
  onClose?: () => void;
  className?: string;
  style?: React.CSSProperties;
  children?: React.ReactNode;
};

const variantToColors: Record<
  AlertVariant,
  { border: string; bg: string; text: string; icon: string }
> = {
  info: {
    border: "var(--color-semantic-info)",
    bg: "rgba(59, 130, 246, 0.08)",
    text: "var(--color-neutral-300)",
    icon: "var(--color-semantic-info)",
  },
  success: {
    border: "var(--color-semantic-success)",
    bg: "rgba(22, 163, 74, 0.08)",
    text: "var(--color-neutral-300)",
    icon: "var(--color-semantic-success)",
  },
  warning: {
    border: "var(--color-semantic-warning)",
    bg: "rgba(245, 158, 11, 0.08)",
    text: "var(--color-neutral-300)",
    icon: "var(--color-semantic-warning)",
  },
  error: {
    border: "var(--color-semantic-error)",
    bg: "rgba(239, 68, 68, 0.08)",
    text: "var(--color-neutral-300)",
    icon: "var(--color-semantic-error)",
  },
};

export function Alert({
  variant = "info",
  title,
  description,
  icon,
  dismissible = false,
  onClose,
  className = "",
  style,
  children,
}: AlertProps) {
  const palette = variantToColors[variant];

  return (
    <div
      role="alert"
      style={{
        display: "flex",
        alignItems: "flex-start",
        gap: "12px",
        padding: "12px 14px",
        borderRadius: "var(--radius-md)",
        border: `1px solid ${palette.border}`,
        background: palette.bg,
        color: palette.text,
        fontFamily: "var(--font-body)",
        ...style,
      }}
      className={className}
    >
      <div style={{ lineHeight: 0, color: palette.icon }}>
        {icon || (
          <svg
            width="20"
            height="20"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
          >
            {variant === "error" ? (
              <>
                <circle cx="12" cy="12" r="10" />
                <line x1="15" y1="9" x2="9" y2="15" />
                <line x1="9" y1="9" x2="15" y2="15" />
              </>
            ) : variant === "warning" ? (
              <>
                <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z" />
                <line x1="12" y1="9" x2="12" y2="13" />
                <line x1="12" y1="17" x2="12.01" y2="17" />
              </>
            ) : variant === "success" ? (
              <>
                <circle cx="12" cy="12" r="10" />
                <path d="m9 12 2 2 4-4" />
              </>
            ) : (
              <>
                <circle cx="12" cy="12" r="10" />
                <line x1="12" y1="8" x2="12" y2="12" />
                <line x1="12" y1="16" x2="12.01" y2="16" />
              </>
            )}
          </svg>
        )}
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        {title && (
          <div
            style={{
              color: "#FFFFFF",
              fontWeight: 600,
              fontSize: "14px",
              marginBottom: description || children ? "4px" : 0,
            }}
          >
            {title}
          </div>
        )}
        {(description || children) && (
          <div style={{ fontSize: "13px", lineHeight: 1.5 }}>
            {description || children}
          </div>
        )}
      </div>
      {dismissible && (
        <button
          aria-label="Close"
          onClick={onClose}
          style={{
            background: "transparent",
            border: "none",
            color: palette.icon,
            cursor: "pointer",
            padding: 0,
            lineHeight: 0,
          }}
        >
          <svg
            width="18"
            height="18"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
          >
            <line x1="15" y1="9" x2="9" y2="15" />
            <line x1="9" y1="9" x2="15" y2="15" />
          </svg>
        </button>
      )}
    </div>
  );
}
