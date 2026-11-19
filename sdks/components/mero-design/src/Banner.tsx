import React, { useState, useEffect } from "react";
import { Button } from "./Button";
import { Icon } from "./Icon";

export type BannerVariant =
  | "info"
  | "success"
  | "warning"
  | "error"
  | "announcement"
  | "promotion";

export type BannerSize = "sm" | "md" | "lg";

export type BannerPosition = "top" | "bottom" | "inline";

export interface BannerAction {
  label: string;
  onClick: () => void;
  variant?: "primary" | "secondary" | "success" | "warning" | "error" | "info";
  disabled?: boolean;
}

export interface BannerProps {
  /** Banner variant */
  variant?: BannerVariant;
  /** Banner size */
  size?: BannerSize;
  /** Banner position */
  position?: BannerPosition;
  /** Banner title */
  title?: string;
  /** Banner description */
  description?: string;
  /** Custom icon */
  icon?: React.ReactNode;
  /** Show close button */
  dismissible?: boolean;
  /** Close handler */
  onClose?: () => void;
  /** Auto dismiss after delay (in milliseconds) */
  autoDismiss?: number;
  /** Action buttons */
  actions?: BannerAction[];
  /** Whether banner is visible */
  visible?: boolean;
  /** Custom content */
  children?: React.ReactNode;
  /** Additional CSS class */
  className?: string;
  /** Custom styles */
  style?: React.CSSProperties;
  /** Whether to show as sticky */
  sticky?: boolean;
  /** Whether to show with animation */
  animated?: boolean;
  /** Custom background color */
  backgroundColor?: string;
  /** Custom text color */
  textColor?: string;
  /** Custom border color */
  borderColor?: string;
}

const variantToColors: Record<
  BannerVariant,
  {
    border: string;
    bg: string;
    text: string;
    icon: string;
    title: string;
  }
> = {
  info: {
    border: "var(--color-semantic-info)",
    bg: "rgba(59, 130, 246, 0.08)",
    text: "var(--color-neutral-300)",
    icon: "var(--color-semantic-info)",
    title: "#FFFFFF",
  },
  success: {
    border: "var(--color-semantic-success)",
    bg: "rgba(22, 163, 74, 0.08)",
    text: "var(--color-neutral-300)",
    icon: "var(--color-semantic-success)",
    title: "#FFFFFF",
  },
  warning: {
    border: "var(--color-semantic-warning)",
    bg: "rgba(245, 158, 11, 0.08)",
    text: "var(--color-neutral-300)",
    icon: "var(--color-semantic-warning)",
    title: "#FFFFFF",
  },
  error: {
    border: "var(--color-semantic-error)",
    bg: "rgba(239, 68, 68, 0.08)",
    text: "var(--color-neutral-300)",
    icon: "var(--color-semantic-error)",
    title: "#FFFFFF",
  },
  announcement: {
    border: "var(--color-brand-600)",
    bg: "rgba(139, 92, 246, 0.08)",
    text: "var(--color-neutral-300)",
    icon: "var(--color-brand-600)",
    title: "#FFFFFF",
  },
  promotion: {
    border: "var(--color-semantic-warning)",
    bg: "linear-gradient(135deg, rgba(245, 158, 11, 0.1) 0%, rgba(251, 191, 36, 0.1) 100%)",
    text: "var(--color-neutral-300)",
    icon: "var(--color-semantic-warning)",
    title: "#FFFFFF",
  },
};

const sizeToStyles: Record<
  BannerSize,
  {
    padding: string;
    fontSize: string;
    iconSize: number;
    gap: string;
  }
> = {
  sm: {
    padding: "8px 12px",
    fontSize: "13px",
    iconSize: 16,
    gap: "8px",
  },
  md: {
    padding: "12px 16px",
    fontSize: "14px",
    iconSize: 20,
    gap: "12px",
  },
  lg: {
    padding: "16px 20px",
    fontSize: "15px",
    iconSize: 24,
    gap: "16px",
  },
};

const positionToStyles: Record<BannerPosition, React.CSSProperties> = {
  top: {
    position: "fixed",
    top: 0,
    left: 0,
    right: 0,
    zIndex: 1000,
    width: "100%",
  },
  bottom: {
    position: "fixed",
    bottom: 0,
    left: 0,
    right: 0,
    zIndex: 1000,
    width: "100%",
  },
  inline: {
    position: "relative",
    width: "100%",
  },
};

export const Banner: React.FC<BannerProps> = ({
  variant = "info",
  size = "md",
  position = "inline",
  title,
  description,
  icon,
  dismissible = false,
  onClose,
  autoDismiss,
  actions = [],
  visible = true,
  children,
  className = "",
  style = {},
  sticky = false,
  animated = true,
  backgroundColor,
  textColor,
  borderColor,
}) => {
  const [isVisible, setIsVisible] = useState(visible);
  const [isAnimating, setIsAnimating] = useState(false);

  const palette = variantToColors[variant];
  const sizeStyles = sizeToStyles[size];
  const positionStyles = positionToStyles[position];

  // Auto dismiss functionality
  useEffect(() => {
    if (autoDismiss && isVisible) {
      const timer = setTimeout(() => {
        handleClose();
      }, autoDismiss);

      return () => clearTimeout(timer);
    }
  }, [autoDismiss, isVisible]);

  // Handle close with animation
  const handleClose = () => {
    if (animated) {
      setIsAnimating(true);
      setTimeout(() => {
        setIsVisible(false);
        onClose?.();
      }, 200);
    } else {
      setIsVisible(false);
      onClose?.();
    }
  };

  // Handle action click
  const handleActionClick = (action: BannerAction) => {
    if (!action.disabled) {
      action.onClick();
    }
  };

  // Get default icon based on variant
  const getDefaultIcon = () => {
    const iconProps = {
      width: sizeStyles.iconSize,
      height: sizeStyles.iconSize,
      viewBox: "0 0 24 24",
      fill: "none",
      stroke: "currentColor",
      strokeWidth: "2",
    };

    switch (variant) {
      case "error":
        return (
          <svg {...iconProps}>
            <circle cx="12" cy="12" r="10" />
            <line x1="15" y1="9" x2="9" y2="15" />
            <line x1="9" y1="9" x2="15" y2="15" />
          </svg>
        );
      case "warning":
        return (
          <svg {...iconProps}>
            <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z" />
            <line x1="12" y1="9" x2="12" y2="13" />
            <line x1="12" y1="17" x2="12.01" y2="17" />
          </svg>
        );
      case "success":
        return (
          <svg {...iconProps}>
            <circle cx="12" cy="12" r="10" />
            <path d="m9 12 2 2 4-4" />
          </svg>
        );
      case "announcement":
        return (
          <svg {...iconProps}>
            <path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z" />
          </svg>
        );
      case "promotion":
        return (
          <svg {...iconProps}>
            <path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z" />
          </svg>
        );
      default:
        return (
          <svg {...iconProps}>
            <circle cx="12" cy="12" r="10" />
            <line x1="12" y1="8" x2="12" y2="12" />
            <line x1="12" y1="16" x2="12.01" y2="16" />
          </svg>
        );
    }
  };

  if (!isVisible) return null;

  const bannerStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "flex-start",
    gap: sizeStyles.gap,
    padding: sizeStyles.padding,
    borderRadius: "var(--radius-md)",
    border: `1px solid ${borderColor || palette.border}`,
    background: backgroundColor || palette.bg,
    color: textColor || palette.text,
    fontFamily: "var(--font-body)",
    fontSize: sizeStyles.fontSize,
    lineHeight: 1.5,
    transition: animated ? "all 0.2s ease-in-out" : "none",
    transform: isAnimating ? "translateY(-100%)" : "translateY(0)",
    opacity: isAnimating ? 0 : 1,
    ...(sticky ? { position: "sticky", top: 0, zIndex: 100 } : {}),
    ...positionStyles,
    ...style,
  };

  return (
    <div
      role="banner"
      aria-live="polite"
      style={bannerStyle}
      className={className}
    >
      {/* Icon */}
      <div
        style={{
          lineHeight: 0,
          color: palette.icon,
          flexShrink: 0,
          marginTop: title ? "2px" : "0px",
        }}
      >
        {icon || getDefaultIcon()}
      </div>

      {/* Content */}
      <div style={{ flex: 1, minWidth: 0 }}>
        {title && (
          <div
            style={{
              color: palette.title,
              fontWeight: 600,
              fontSize: sizeStyles.fontSize,
              marginBottom: description || children ? "4px" : 0,
            }}
          >
            {title}
          </div>
        )}
        {(description || children) && (
          <div
            style={{
              fontSize: sizeStyles.fontSize,
              lineHeight: 1.5,
              color: textColor || palette.text,
            }}
          >
            {description || children}
          </div>
        )}
      </div>

      {/* Actions */}
      {actions.length > 0 && (
        <div
          style={{
            display: "flex",
            gap: "8px",
            alignItems: "center",
            flexShrink: 0,
            marginLeft: "8px",
          }}
        >
          {actions.map((action, index) => (
            <Button
              key={index}
              variant={action.variant || "secondary"}
              onClick={() => handleActionClick(action)}
              disabled={action.disabled}
              style={{
                fontSize:
                  size === "sm" ? "12px" : size === "lg" ? "14px" : "13px",
                padding:
                  size === "sm"
                    ? "4px 8px"
                    : size === "lg"
                      ? "8px 16px"
                      : "6px 12px",
                minHeight: "auto",
              }}
            >
              {action.label}
            </Button>
          ))}
        </div>
      )}

      {/* Close button */}
      {dismissible && (
        <button
          aria-label="Close banner"
          onClick={handleClose}
          style={{
            background: "transparent",
            border: "none",
            color: palette.icon,
            cursor: "pointer",
            padding: "4px",
            lineHeight: 0,
            flexShrink: 0,
            borderRadius: "4px",
            transition: "background-color 0.2s ease",
            marginLeft: "8px",
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.backgroundColor = "rgba(255, 255, 255, 0.1)";
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.backgroundColor = "transparent";
          }}
        >
          <Icon name="close" size="sm" />
        </button>
      )}
    </div>
  );
};

// Convenience components for common banner types
export const InfoBanner: React.FC<Omit<BannerProps, "variant">> = (props) => (
  <Banner {...props} variant="info" />
);

export const SuccessBanner: React.FC<Omit<BannerProps, "variant">> = (
  props,
) => <Banner {...props} variant="success" />;

export const WarningBanner: React.FC<Omit<BannerProps, "variant">> = (
  props,
) => <Banner {...props} variant="warning" />;

export const ErrorBanner: React.FC<Omit<BannerProps, "variant">> = (props) => (
  <Banner {...props} variant="error" />
);

export const AnnouncementBanner: React.FC<Omit<BannerProps, "variant">> = (
  props,
) => <Banner {...props} variant="announcement" />;

export const PromotionBanner: React.FC<Omit<BannerProps, "variant">> = (
  props,
) => <Banner {...props} variant="promotion" />;

export default Banner;
