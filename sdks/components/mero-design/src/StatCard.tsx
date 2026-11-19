import React from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Card } from "./Card";
import { Icon } from "./Icon";

export interface StatCardProps {
  title: string;
  value: string | number;
  subtitle?: string;
  trend?: {
    value: number;
    direction: "up" | "down" | "neutral";
    period?: string;
  };
  icon?: React.ReactNode;
  color?: "primary" | "success" | "warning" | "error" | "info" | "neutral";
  size?: "small" | "medium" | "large";
  loading?: boolean;
  className?: string;
  style?: React.CSSProperties;
  onClick?: () => void;
}

export function StatCard({
  title,
  value,
  subtitle,
  trend,
  icon,
  color = "primary",
  size = "medium",
  loading = false,
  className = "",
  style = {},
  onClick,
}: StatCardProps) {
  const colorStyles = {
    primary: {
      background: `linear-gradient(135deg, ${tokens.color.brand[600].value}20, ${tokens.color.brand[600].value}10)`,
      border: `1px solid ${tokens.color.brand[600].value}40`,
      iconColor: tokens.color.brand[600].value,
      valueColor: "#FFFFFF",
    },
    success: {
      background: `linear-gradient(135deg, ${tokens.color.semantic.success.value}20, ${tokens.color.semantic.success.value}10)`,
      border: `1px solid ${tokens.color.semantic.success.value}40`,
      iconColor: tokens.color.semantic.success.value,
      valueColor: "#FFFFFF",
    },
    warning: {
      background: `linear-gradient(135deg, ${tokens.color.semantic.warning.value}20, ${tokens.color.semantic.warning.value}10)`,
      border: `1px solid ${tokens.color.semantic.warning.value}40`,
      iconColor: tokens.color.semantic.warning.value,
      valueColor: "#FFFFFF",
    },
    error: {
      background: `linear-gradient(135deg, ${tokens.color.semantic.error.value}20, ${tokens.color.semantic.error.value}10)`,
      border: `1px solid ${tokens.color.semantic.error.value}40`,
      iconColor: tokens.color.semantic.error.value,
      valueColor: "#FFFFFF",
    },
    info: {
      background: `linear-gradient(135deg, ${tokens.color.semantic.info.value}20, ${tokens.color.semantic.info.value}10)`,
      border: `1px solid ${tokens.color.semantic.info.value}40`,
      iconColor: tokens.color.semantic.info.value,
      valueColor: "#FFFFFF",
    },
    neutral: {
      background: `linear-gradient(135deg, ${tokens.color.neutral[600].value}20, ${tokens.color.neutral[600].value}10)`,
      border: `1px solid ${tokens.color.neutral[600].value}40`,
      iconColor: tokens.color.neutral[400].value,
      valueColor: "#FFFFFF",
    },
  };

  const sizeStyles = {
    small: {
      padding: "16px",
      titleSize: "12px",
      valueSize: "20px",
      subtitleSize: "10px",
      iconSize: "16px",
    },
    medium: {
      padding: "20px",
      titleSize: "14px",
      valueSize: "28px",
      subtitleSize: "12px",
      iconSize: "20px",
    },
    large: {
      padding: "24px",
      titleSize: "16px",
      valueSize: "36px",
      subtitleSize: "14px",
      iconSize: "24px",
    },
  };

  const currentColorStyle = colorStyles[color];
  const currentSizeStyle = sizeStyles[size];

  const getTrendIcon = () => {
    if (!trend) return null;

    switch (trend.direction) {
      case "up":
        return "↗";
      case "down":
        return "↘";
      case "neutral":
        return "→";
      default:
        return null;
    }
  };

  const getTrendColor = () => {
    if (!trend) return tokens.color.neutral[400].value;

    switch (trend.direction) {
      case "up":
        return tokens.color.semantic.success.value;
      case "down":
        return tokens.color.semantic.error.value;
      case "neutral":
        return tokens.color.neutral[400].value;
      default:
        return tokens.color.neutral[400].value;
    }
  };

  if (loading) {
    return (
      <Card
        className={className}
        style={{
          ...currentColorStyle,
          ...currentSizeStyle,
          ...style,
          cursor: "default",
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
          }}
        >
          <div style={{ flex: 1 }}>
            <div
              style={{
                height: currentSizeStyle.titleSize,
                background: tokens.color.neutral[700].value,
                borderRadius: "4px",
                marginBottom: "8px",
                width: "60%",
              }}
            />
            <div
              style={{
                height: currentSizeStyle.valueSize,
                background: tokens.color.neutral[700].value,
                borderRadius: "4px",
                marginBottom: "4px",
                width: "40%",
              }}
            />
            <div
              style={{
                height: currentSizeStyle.subtitleSize,
                background: tokens.color.neutral[700].value,
                borderRadius: "4px",
                width: "30%",
              }}
            />
          </div>
          <div
            style={{
              width: currentSizeStyle.iconSize,
              height: currentSizeStyle.iconSize,
              background: tokens.color.neutral[700].value,
              borderRadius: "50%",
            }}
          />
        </div>
      </Card>
    );
  }

  return (
    <div
      className={className}
      style={{
        cursor: onClick ? "pointer" : "default",
        transition: "all 0.2s ease",
      }}
      onClick={onClick}
      onMouseEnter={(e: React.MouseEvent<HTMLDivElement>) => {
        if (onClick) {
          e.currentTarget.style.transform = "translateY(-2px)";
          e.currentTarget.style.boxShadow = "0 8px 25px rgba(0, 0, 0, 0.3)";
        }
      }}
      onMouseLeave={(e: React.MouseEvent<HTMLDivElement>) => {
        if (onClick) {
          e.currentTarget.style.transform = "translateY(0)";
          e.currentTarget.style.boxShadow = "none";
        }
      }}
    >
      <Card
        style={{
          ...currentColorStyle,
          ...currentSizeStyle,
          ...style,
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "flex-start",
            justifyContent: "space-between",
          }}
        >
          <div style={{ flex: 1, minWidth: 0 }}>
            <div
              style={{
                fontSize: currentSizeStyle.titleSize,
                color: tokens.color.neutral[300].value,
                fontWeight: 500,
                marginBottom: "8px",
                lineHeight: 1.2,
              }}
            >
              {title}
            </div>

            <div
              style={{
                fontSize: currentSizeStyle.valueSize,
                color: currentColorStyle.valueColor,
                fontWeight: 700,
                marginBottom: "4px",
                lineHeight: 1.1,
                fontFamily: "var(--font-heading)",
              }}
            >
              {value}
            </div>

            {subtitle && (
              <div
                style={{
                  fontSize: currentSizeStyle.subtitleSize,
                  color: tokens.color.neutral[400].value,
                  lineHeight: 1.2,
                }}
              >
                {subtitle}
              </div>
            )}

            {trend && (
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: "4px",
                  marginTop: "8px",
                }}
              >
                <span
                  style={{
                    fontSize: currentSizeStyle.subtitleSize,
                    color: getTrendColor(),
                    fontWeight: 600,
                  }}
                >
                  {getTrendIcon()} {Math.abs(trend.value)}%
                </span>
                {trend.period && (
                  <span
                    style={{
                      fontSize: currentSizeStyle.subtitleSize,
                      color: tokens.color.neutral[400].value,
                    }}
                  >
                    {trend.period}
                  </span>
                )}
              </div>
            )}
          </div>

          {icon && (
            <div
              style={{
                color: currentColorStyle.iconColor,
                fontSize: currentSizeStyle.iconSize,
                marginLeft: "12px",
                flexShrink: 0,
              }}
            >
              {icon}
            </div>
          )}
        </div>
      </Card>
    </div>
  );
}
