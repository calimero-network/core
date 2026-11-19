import React from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Icon } from "./Icon";

interface TimelineItem {
  id: string;
  title: string;
  description?: string;
  timestamp?: string;
  status?: "completed" | "current" | "pending" | "error";
  icon?: React.ReactNode;
  color?: "primary" | "success" | "warning" | "error" | "neutral";
}

interface TimelineProps {
  items: TimelineItem[];
  orientation?: "vertical" | "horizontal";
  size?: "sm" | "md" | "lg";
  variant?: "default" | "minimal" | "detailed";
  className?: string;
  style?: React.CSSProperties;
}

export const Timeline: React.FC<TimelineProps> = ({
  items,
  orientation = "vertical",
  size = "md",
  variant = "default",
  className = "",
  style = {},
}) => {
  const sizeStyles = {
    sm: {
      itemSpacing: tokens.space["4"].value,
      iconSize: 16,
      fontSize: "12px",
      lineWidth: 2,
    },
    md: {
      itemSpacing: tokens.space["6"].value,
      iconSize: 20,
      fontSize: "14px",
      lineWidth: 2,
    },
    lg: {
      itemSpacing: "2rem",
      iconSize: 24,
      fontSize: "16px",
      lineWidth: 3,
    },
  };

  const statusColors = {
    completed: tokens.color.semantic.success.value,
    current: tokens.color.brand[600].value,
    pending: tokens.color.neutral[300].value,
    error: tokens.color.semantic.error.value,
  };

  const colorVariants = {
    primary: tokens.color.brand[600].value,
    success: tokens.color.semantic.success.value,
    warning: tokens.color.semantic.warning.value,
    error: tokens.color.semantic.error.value,
    neutral: tokens.color.neutral[400].value,
  };

  const getItemColor = (item: TimelineItem): string => {
    if (item.color) {
      return colorVariants[item.color];
    }
    return statusColors[item.status || "pending"];
  };

  const getDefaultIcon = (item: TimelineItem): React.ReactNode => {
    switch (item.status) {
      case "completed":
        return <Icon name="check-circle" size="sm" color="current" />;
      case "current":
        return <Icon name="activity" size="sm" color="current" />;
      case "error":
        return <Icon name="x-circle" size="sm" color="current" />;
      default:
        return <Icon name="circle" size="sm" color="current" />;
    }
  };

  const containerStyle: React.CSSProperties = {
    display: "flex",
    flexDirection: orientation === "vertical" ? "column" : "row",
    gap: sizeStyles[size].itemSpacing,
    ...style,
  };

  const itemStyle: React.CSSProperties = {
    display: "flex",
    flexDirection: orientation === "vertical" ? "row" : "column",
    alignItems: orientation === "vertical" ? "flex-start" : "center",
    position: "relative",
  };

  const contentStyle: React.CSSProperties = {
    flex: 1,
    marginLeft: orientation === "vertical" ? tokens.space["4"].value : 0,
    marginTop: orientation === "horizontal" ? tokens.space["2"].value : 0,
  };

  const iconContainerStyle = (item: TimelineItem): React.CSSProperties => ({
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    width: sizeStyles[size].iconSize + 8,
    height: sizeStyles[size].iconSize + 8,
    borderRadius: "50%",
    backgroundColor: getItemColor(item),
    color: tokens.color.neutral[200].value,
    flexShrink: 0,
    position: "relative",
    zIndex: 2,
  });

  const lineStyle = (index: number): React.CSSProperties => {
    const isLast = index === items.length - 1;
    const color = getItemColor(items[index]);

    if (orientation === "vertical") {
      return {
        position: "absolute",
        left:
          (sizeStyles[size].iconSize + 8) / 2 - sizeStyles[size].lineWidth / 2,
        top: sizeStyles[size].iconSize + 8,
        width: sizeStyles[size].lineWidth,
        height: isLast ? 0 : `calc(100% + ${sizeStyles[size].itemSpacing})`,
        backgroundColor: color,
        zIndex: 1,
      };
    } else {
      return {
        position: "absolute",
        top:
          (sizeStyles[size].iconSize + 8) / 2 - sizeStyles[size].lineWidth / 2,
        left: sizeStyles[size].iconSize + 8,
        height: sizeStyles[size].lineWidth,
        width: isLast ? 0 : `calc(100% + ${sizeStyles[size].itemSpacing})`,
        backgroundColor: color,
        zIndex: 1,
      };
    }
  };

  const titleStyle: React.CSSProperties = {
    fontSize: sizeStyles[size].fontSize,
    fontWeight: 600,
    color: tokens.color.background.primary.value,
    marginBottom: variant === "detailed" ? "4px" : 0,
  };

  const descriptionStyle: React.CSSProperties = {
    fontSize: sizeStyles[size].fontSize,
    color: tokens.color.neutral[300].value,
    lineHeight: 1.5,
    marginBottom: variant === "detailed" ? tokens.space["2"].value : 0,
  };

  const timestampStyle: React.CSSProperties = {
    fontSize: "12px",
    color: tokens.color.neutral[300].value,
    fontStyle: "italic",
  };

  return (
    <div className={className} style={containerStyle}>
      {items.map((item, index) => (
        <div key={item.id} style={itemStyle}>
          <div style={iconContainerStyle(item)}>
            {item.icon || getDefaultIcon(item)}
          </div>
          {index < items.length - 1 && <div style={lineStyle(index)} />}
          <div style={contentStyle}>
            <div style={titleStyle}>{item.title}</div>
            {variant === "detailed" && item.description && (
              <div style={descriptionStyle}>{item.description}</div>
            )}
            {variant === "detailed" && item.timestamp && (
              <div style={timestampStyle}>{item.timestamp}</div>
            )}
          </div>
        </div>
      ))}
    </div>
  );
};
