import React from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Icon } from "./Icon";

export type BadgeVariant =
  | "default"
  | "success"
  | "warning"
  | "error"
  | "info"
  | "outline"
  | "neutral";

export type BadgeProps = {
  children?: React.ReactNode;
  variant?: BadgeVariant;
  dot?: boolean;
  icon?: string;
  size?: "sm" | "md" | "lg";
  className?: string;
  style?: React.CSSProperties;
};

// Status-specific props for backward compatibility
export type StatusBadgeProps = {
  status:
    | "active"
    | "inactive"
    | "pending"
    | "error"
    | "success"
    | "warning"
    | "info";
  text?: string;
  size?: "sm" | "md" | "lg";
  showIcon?: boolean;
  className?: string;
  style?: React.CSSProperties;
};

const variantToStyles: Record<
  BadgeVariant,
  { bg: string; color: string; border: string }
> = {
  default: {
    bg: "var(--color-brand-100)",
    color: "#0B0B0B",
    border: "transparent",
  },
  success: {
    bg: "rgba(22, 163, 74, 0.15)",
    color: "var(--color-semantic-success)",
    border: "transparent",
  },
  warning: {
    bg: "rgba(245, 158, 11, 0.15)",
    color: "var(--color-semantic-warning)",
    border: "transparent",
  },
  error: {
    bg: "rgba(239, 68, 68, 0.15)",
    color: "var(--color-semantic-error)",
    border: "transparent",
  },
  info: {
    bg: "rgba(59, 130, 246, 0.15)",
    color: "var(--color-semantic-info)",
    border: "transparent",
  },
  outline: {
    bg: "transparent",
    color: "var(--color-neutral-300)",
    border: "var(--color-neutral-600)",
  },
  neutral: {
    bg: "var(--color-neutral-700)",
    color: "var(--color-neutral-300)",
    border: "var(--color-neutral-600)",
  },
};

const sizeStyles = {
  sm: {
    padding: "2px 6px",
    fontSize: "11px",
    borderRadius: "4px",
    iconSize: "xs" as const,
    gap: "4px",
  },
  md: {
    padding: "2px 8px",
    fontSize: "12px",
    borderRadius: "6px",
    iconSize: "xs" as const,
    gap: "6px",
  },
  lg: {
    padding: "4px 12px",
    fontSize: "14px",
    borderRadius: "8px",
    iconSize: "sm" as const,
    gap: "8px",
  },
};

const statusConfig = {
  active: { variant: "success" as const, icon: "check-circle" },
  inactive: { variant: "neutral" as const, icon: "x-circle" },
  pending: { variant: "warning" as const, icon: "clock" },
  error: { variant: "error" as const, icon: "alert-circle" },
  success: { variant: "success" as const, icon: "check-circle" },
  warning: { variant: "warning" as const, icon: "alert-triangle" },
  info: { variant: "info" as const, icon: "info" },
};

export function Badge({
  children,
  variant = "default",
  dot = false,
  icon,
  size = "md",
  className = "",
  style = {},
}: BadgeProps) {
  const palette = variantToStyles[variant];
  const currentSize = sizeStyles[size];

  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: currentSize.gap,
        padding: currentSize.padding,
        borderRadius: currentSize.borderRadius,
        background: palette.bg,
        color: palette.color,
        border: `1px solid ${palette.border}`,
        fontSize: currentSize.fontSize,
        fontWeight: 600,
        lineHeight: 1,
        fontFamily: "var(--font-body)",
        ...style,
      }}
      className={className}
    >
      {dot && (
        <span
          style={{
            width: "6px",
            height: "6px",
            borderRadius: "50%",
            background: palette.color,
          }}
        />
      )}
      {icon && <Icon name={icon} size={currentSize.iconSize} color="current" />}
      {children}
    </span>
  );
}

// StatusBadge component for backward compatibility
export function StatusBadge({
  status,
  text,
  size = "md",
  showIcon = true,
  className = "",
  style = {},
}: StatusBadgeProps) {
  const config = statusConfig[status];

  return (
    <Badge
      variant={config.variant}
      size={size}
      icon={showIcon ? config.icon : undefined}
      className={className}
      style={style}
    >
      {text || status.charAt(0).toUpperCase() + status.slice(1)}
    </Badge>
  );
}
