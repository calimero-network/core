import React from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Icon } from "./Icon";
import { Text, Heading } from "./Typography";
import { Button } from "./Button";

export interface EmptyStateProps {
  icon?: React.ReactNode;
  title: string;
  description?: string;
  action?: React.ReactNode;
  size?: "sm" | "md" | "lg";
  variant?: "default" | "minimal" | "card";
  type?: "empty" | "error" | "not-found" | "loading";
  className?: string;
  style?: React.CSSProperties;
}

// Legacy ErrorView props for backward compatibility
export interface ErrorViewProps {
  title?: string;
  message?: string;
  actionLabel?: string;
  onAction?: () => void;
  variant?: "error" | "empty" | "not-found";
  icon?: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
  showAction?: boolean;
}

export function EmptyState({
  icon,
  title,
  description,
  action,
  size = "md",
  variant = "default",
  type = "empty",
  className = "",
  style = {},
}: EmptyStateProps) {
  const sizeStyles = {
    sm: {
      padding: "24px",
      iconSize: "md" as const,
      titleSize: "md" as const,
      descriptionSize: "sm" as const,
      gap: "12px",
    },
    md: {
      padding: "32px",
      iconSize: "lg" as const,
      titleSize: "lg" as const,
      descriptionSize: "md" as const,
      gap: "16px",
    },
    lg: {
      padding: "48px",
      iconSize: "xl" as const,
      titleSize: "xl" as const,
      descriptionSize: "lg" as const,
      gap: "20px",
    },
  };

  const currentSize = sizeStyles[size];

  // Type-specific configurations
  const getTypeConfig = () => {
    switch (type) {
      case "error":
        return {
          defaultIcon: (
            <Icon
              name="alert-circle"
              size={currentSize.iconSize}
              color="error"
            />
          ),
          titleColor: tokens.color.semantic.error.value,
          descriptionColor: tokens.color.neutral[300].value,
        };
      case "not-found":
        return {
          defaultIcon: (
            <Icon name="search" size={currentSize.iconSize} color="muted" />
          ),
          titleColor: tokens.color.neutral[200].value,
          descriptionColor: tokens.color.neutral[400].value,
        };
      case "loading":
        return {
          defaultIcon: (
            <Icon name="loader" size={currentSize.iconSize} color="muted" />
          ),
          titleColor: tokens.color.neutral[200].value,
          descriptionColor: tokens.color.neutral[400].value,
        };
      default: // empty
        return {
          defaultIcon: (
            <Icon name="inbox" size={currentSize.iconSize} color="muted" />
          ),
          titleColor: tokens.color.neutral[200].value,
          descriptionColor: tokens.color.neutral[400].value,
        };
    }
  };

  const typeConfig = getTypeConfig();
  const displayIcon = icon || typeConfig.defaultIcon;

  const containerStyles: React.CSSProperties = {
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    justifyContent: "center",
    textAlign: "center",
    padding: currentSize.padding,
    gap: currentSize.gap,
    ...style,
  };

  const cardStyles: React.CSSProperties =
    variant === "card"
      ? {
          background: "var(--color-background-primary)",
          border: `1px solid ${tokens.color.neutral[700].value}`,
          borderRadius: "12px",
          ...containerStyles,
        }
      : containerStyles;

  const minimalStyles: React.CSSProperties =
    variant === "minimal"
      ? {
          padding: "16px",
          ...containerStyles,
        }
      : cardStyles;

  return (
    <div className={className} style={minimalStyles}>
      <div
        style={{
          color: typeConfig.titleColor,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        {displayIcon}
      </div>

      <Heading
        size={currentSize.titleSize}
        style={{
          margin: 0,
          color: typeConfig.titleColor,
        }}
      >
        {title}
      </Heading>

      {description && (
        <Text
          size={currentSize.descriptionSize}
          style={{
            margin: 0,
            color: typeConfig.descriptionColor,
            maxWidth: variant === "minimal" ? "300px" : "400px",
          }}
        >
          {description}
        </Text>
      )}

      {action && <div style={{ marginTop: "8px" }}>{action}</div>}
    </div>
  );
}

// Predefined empty states for common use cases
export function EmptySearchState({
  searchTerm,
  onClear,
  ...props
}: Omit<EmptyStateProps, "icon" | "title" | "description"> & {
  searchTerm?: string;
  onClear?: () => void;
}) {
  return (
    <EmptyState
      icon={<Icon name="search" size="lg" color="muted" />}
      title="No results found"
      description={
        searchTerm
          ? `No results found for "${searchTerm}"`
          : "Try adjusting your search terms"
      }
      action={
        onClear && (
          <Button variant="secondary" onClick={onClear}>
            Clear search
          </Button>
        )
      }
      {...props}
    />
  );
}

export function EmptyDataState({
  onRefresh,
  ...props
}: Omit<EmptyStateProps, "icon" | "title" | "description"> & {
  onRefresh?: () => void;
}) {
  return (
    <EmptyState
      icon={<Icon name="database" size="lg" color="muted" />}
      title="No data available"
      description="There's no data to display at the moment"
      action={
        onRefresh && (
          <Button variant="primary" onClick={onRefresh}>
            Refresh
          </Button>
        )
      }
      {...props}
    />
  );
}

// Legacy ErrorView components for backward compatibility
export function ErrorView({
  title = "Something went wrong",
  message = "An unexpected error occurred. Please try again.",
  actionLabel = "Try Again",
  onAction,
  variant = "error",
  icon,
  className = "",
  style,
  showAction = true,
}: ErrorViewProps) {
  const typeMap = {
    error: "error" as const,
    empty: "empty" as const,
    "not-found": "not-found" as const,
  };

  return (
    <EmptyState
      icon={icon}
      title={title}
      description={message}
      action={
        showAction &&
        onAction && (
          <Button variant="primary" onClick={onAction}>
            {actionLabel}
          </Button>
        )
      }
      type={typeMap[variant]}
      className={className}
      style={style}
    />
  );
}

export function EmptyView(props: Omit<ErrorViewProps, "variant">) {
  return <ErrorView {...props} variant="empty" />;
}

export function NotFoundView(props: Omit<ErrorViewProps, "variant">) {
  return <ErrorView {...props} variant="not-found" />;
}
