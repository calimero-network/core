import React from "react";
import { Tooltip } from "./Tooltip";
import { tokens } from "@calimero-network/mero-tokens";
import { ClockAlert } from "@calimero-network/mero-icons";

export interface CardProps {
  /** Optional CSS class name */
  className?: string;
  
  /** Card content - can be any React node */
  children?: React.ReactNode;
  
  /** Tooltip text to show on the info icon */
  tooltip?: string;
  
  /** Custom icon component for the tooltip trigger */
  tooltipIcon?: React.ComponentType<{
    size?: number | string;
    color?: string;
    strokeWidth?: number | string;
  }>;
  
  /** Border color - defaults to neutral-600 */
  color?: string;
  
  /** Remove border entirely */
  noBorder?: boolean;
  
  /** Border radius variant */
  variant?: "rounded" | "rectangle";
  
  /** Inline styles */
  style?: React.CSSProperties;
  
  /** Convenience prop: card title */
  title?: React.ReactNode;
  
  /** Convenience prop: icon to display with title */
  icon?: React.ReactNode;
  
  /** Convenience prop: description text below title */
  description?: React.ReactNode;
  
  /** Convenience prop: action buttons or elements in header */
  actions?: React.ReactNode;
  
  /** Make card clickable - adds pointer cursor and click handler */
  onClick?: () => void;
}

/**
 * Card component - a flexible container with optional header, content, and actions.
 * 
 * Supports both composition pattern (using CardHeader, CardTitle, CardContent) 
 * and convenience props (title, icon, description, actions).
 */
export function Card({
  className = "",
  children,
  tooltip,
  tooltipIcon: TooltipIcon = ClockAlert,
  color,
  noBorder = false,
  variant = "rounded",
  style,
  title,
  icon,
  description,
  actions,
  onClick,
}: CardProps) {
  const borderColor = color || tokens.color.neutral[600].value;
  const borderRadius = variant === "rectangle" 
    ? tokens.radius.sm.value 
    : tokens.radius.lg.value;

  return (
    <div
      className={className}
      style={{
        background: "transparent",
        border: noBorder ? "none" : `1px solid ${borderColor}`,
        borderRadius,
        padding: tokens.space[3].value,
        width: "100%",
        position: "relative",
        cursor: onClick ? "pointer" : undefined,
        userSelect: onClick ? "none" : undefined,
        transition: onClick ? "opacity 0.15s ease" : undefined,
        ...style,
      }}
      onClick={onClick}
      onMouseEnter={onClick ? (e) => {
        e.currentTarget.style.opacity = "0.9";
      } : undefined}
      onMouseLeave={onClick ? (e) => {
        e.currentTarget.style.opacity = "1";
      } : undefined}
    >
      {/* Tooltip */}
      {tooltip && (
        <div
          style={{
            position: "absolute",
            top: tokens.space[2].value,
            right: tokens.space[2].value,
            cursor: "help",
            zIndex: 1,
          }}
        >
          <Tooltip 
            content={tooltip} 
            placement="left"
          >
            <TooltipIcon 
              size={20} 
              strokeWidth={3} 
              color={borderColor} 
            />
          </Tooltip>
        </div>
      )}

      {/* Convenience header - auto-rendered when title provided */}
      {title !== undefined && (
        <CardHeader>
          <div style={{ 
            display: "flex", 
            alignItems: "center", 
            gap: tokens.space[3].value,
            flex: 1,
            minWidth: 0, // Allows truncation
          }}>
            {icon && <span style={{ flexShrink: 0 }}>{icon}</span>}
            <CardTitle>{title}</CardTitle>
          </div>
          {actions && (
            <div style={{ flexShrink: 0, marginLeft: tokens.space[3].value }}>
              {actions}
            </div>
          )}
        </CardHeader>
      )}

      {/* Convenience description */}
      {description !== undefined && (
        <CardContent style={{ marginTop: title !== undefined ? tokens.space[2].value : 0 }}>
          {description}
        </CardContent>
      )}

      {/* Children - allows composition pattern */}
      {children}
    </div>
  );
}

/**
 * CardHeader - horizontal container for title and actions
 */
export interface CardHeaderProps {
  children: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
}

export function CardHeader({ 
  children, 
  className = "",
  style,
}: CardHeaderProps) {
  return (
    <div
      className={className}
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        gap: tokens.space[3].value,
        marginBottom: tokens.space[2].value,
        ...style,
      }}
    >
      {children}
    </div>
  );
}

/**
 * CardTitle - styled title text with truncation
 */
export interface CardTitleProps {
  children: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
  color?: string;
}

export function CardTitle({ 
  children, 
  className = "",
  style,
  color,
}: CardTitleProps) {
  return (
    <div
      className={className}
      style={{
        color: color || tokens.color.neutral[200].value,
        fontWeight: 500,
        fontSize: "14px",
        lineHeight: "20px",
        overflow: "hidden",
        textOverflow: "ellipsis",
        whiteSpace: "nowrap",
        flex: 1,
        minWidth: 0,
        ...style,
      }}
    >
      {children}
    </div>
  );
}

/**
 * CardContent - content area with proper spacing
 */
export interface CardContentProps {
  children: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
  color?: string;
}

export function CardContent({ 
  children, 
  className = "",
  style,
  color,
}: CardContentProps) {
  return (
    <div
      className={className}
      style={{
        color: color || tokens.color.neutral[300].value,
        fontSize: "14px",
        lineHeight: "20px",
        ...style,
      }}
    >
      {children}
    </div>
  );
}
