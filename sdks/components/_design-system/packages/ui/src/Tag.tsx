import React from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Icon } from "./Icon";

export interface TagProps {
  children: string;
  onRemove?: () => void;
  variant?: "default" | "success" | "warning" | "error" | "info" | "neutral";
  size?: "sm" | "md" | "lg";
  disabled?: boolean;
  className?: string;
  style?: React.CSSProperties;
}

export function Tag({
  children,
  onRemove,
  variant = "default",
  size = "md",
  disabled = false,
  className = "",
  style = {},
}: TagProps) {
  const sizeStyles = {
    sm: {
      padding: "2px 6px",
      fontSize: "11px",
      borderRadius: "4px",
      iconSize: "xs" as const,
      gap: "4px",
    },
    md: {
      padding: "4px 8px",
      fontSize: "12px",
      borderRadius: "6px",
      iconSize: "xs" as const,
      gap: "6px",
    },
    lg: {
      padding: "6px 12px",
      fontSize: "14px",
      borderRadius: "8px",
      iconSize: "sm" as const,
      gap: "8px",
    },
  };

  const variantStyles = {
    default: {
      background: tokens.color.neutral[700].value,
      color: tokens.color.neutral[200].value,
      border: `1px solid ${tokens.color.neutral[600].value}`,
    },
    success: {
      background: tokens.color.semantic.success.value + "20",
      color: tokens.color.semantic.success.value,
      border: `1px solid ${tokens.color.semantic.success.value}40`,
    },
    warning: {
      background: tokens.color.semantic.warning.value + "20",
      color: tokens.color.semantic.warning.value,
      border: `1px solid ${tokens.color.semantic.warning.value}40`,
    },
    error: {
      background: tokens.color.semantic.error.value + "20",
      color: tokens.color.semantic.error.value,
      border: `1px solid ${tokens.color.semantic.error.value}40`,
    },
    info: {
      background: tokens.color.brand[600].value + "20",
      color: tokens.color.brand[600].value,
      border: `1px solid ${tokens.color.brand[600].value}40`,
    },
    neutral: {
      background: tokens.color.neutral[800].value,
      color: tokens.color.neutral[300].value,
      border: `1px solid ${tokens.color.neutral[700].value}`,
    },
  };

  const currentSize = sizeStyles[size];
  const currentVariant = variantStyles[variant];

  const tagStyles: React.CSSProperties = {
    display: "inline-flex",
    alignItems: "center",
    gap: currentSize.gap,
    padding: currentSize.padding,
    fontSize: currentSize.fontSize,
    fontWeight: 500,
    borderRadius: currentSize.borderRadius,
    ...currentVariant,
    opacity: disabled ? 0.6 : 1,
    cursor: disabled ? "not-allowed" : "default",
    transition: "all 0.2s ease",
    ...style,
  };

  const removeButtonStyles: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    background: "none",
    border: "none",
    color: "inherit",
    cursor: disabled ? "not-allowed" : "pointer",
    padding: "2px",
    borderRadius: "2px",
    transition: "background-color 0.2s ease",
  };

  return (
    <span className={className} style={tagStyles}>
      <span>{children}</span>
      {onRemove && !disabled && (
        <button
          type="button"
          onClick={onRemove}
          style={removeButtonStyles}
          onMouseEnter={(e) => {
            e.currentTarget.style.backgroundColor = "rgba(255, 255, 255, 0.1)";
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.backgroundColor = "transparent";
          }}
          aria-label={`Remove ${children} tag`}
        >
          <Icon name="x" size={currentSize.iconSize} color="current" />
        </button>
      )}
    </span>
  );
}
