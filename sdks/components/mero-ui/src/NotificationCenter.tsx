import React, { useState, useEffect, useCallback, useMemo } from "react";
import { Button } from "./Button";
import { Icon } from "./Icon";
import { Badge } from "./Badge";

export type NotificationVariant =
  | "info"
  | "success"
  | "warning"
  | "error"
  | "announcement"
  | "promotion";

export type NotificationPriority = "low" | "medium" | "high" | "urgent";

export type NotificationStatus = "unread" | "read" | "archived";

export type NotificationCategory =
  | "system"
  | "user"
  | "security"
  | "feature"
  | "marketing"
  | "maintenance";

export interface Notification {
  id: string;
  title: string;
  description?: string;
  variant?: NotificationVariant;
  priority?: NotificationPriority;
  status?: NotificationStatus;
  category?: NotificationCategory;
  timestamp: Date;
  readAt?: Date;
  archivedAt?: Date;
  icon?: React.ReactNode;
  image?: string;
  actions?: NotificationAction[];
  metadata?: Record<string, any>;
  expiresAt?: Date;
  persistent?: boolean;
}

export interface NotificationAction {
  label: string;
  onClick: (notification: Notification) => void;
  variant?: "primary" | "secondary" | "success" | "warning" | "error" | "info";
  disabled?: boolean;
}

export interface NotificationCenterProps {
  /** Notifications to display */
  notifications: Notification[];
  /** Whether the center is open */
  open?: boolean;
  /** Open/close handler */
  onToggle?: (open: boolean) => void;
  /** Notification click handler */
  onNotificationClick?: (notification: Notification) => void;
  /** Mark as read handler */
  onMarkAsRead?: (notificationId: string) => void;
  /** Mark all as read handler */
  onMarkAllAsRead?: () => void;
  /** Archive notification handler */
  onArchive?: (notificationId: string) => void;
  /** Archive all notifications handler */
  onArchiveAll?: () => void;
  /** Delete notification handler */
  onDelete?: (notificationId: string) => void;
  /** Clear all notifications handler */
  onClearAll?: () => void;
  /** Filter by status */
  statusFilter?: NotificationStatus | "all";
  /** Filter by category */
  categoryFilter?: NotificationCategory | "all";
  /** Filter by priority */
  priorityFilter?: NotificationPriority | "all";
  /** Maximum number of notifications to show */
  maxNotifications?: number;
  /** Whether to show unread count badge */
  showUnreadCount?: boolean;
  /** Whether to group notifications by date */
  groupByDate?: boolean;
  /** Whether to show notification timestamps */
  showTimestamps?: boolean;
  /** Whether to show notification categories */
  showCategories?: boolean;
  /** Whether to show notification priorities */
  showPriorities?: boolean;
  /** Custom empty state message */
  emptyMessage?: string;
  /** Custom empty state icon */
  emptyIcon?: React.ReactNode;
  /** Position of the notification center */
  position?: "top-right" | "top-left" | "bottom-right" | "bottom-left";
  /** Size of the notification center */
  size?: "sm" | "md" | "lg";
  /** Whether to show the center as a dropdown */
  dropdown?: boolean;
  /** Custom trigger element */
  trigger?: React.ReactNode;
  /** Additional CSS class */
  className?: string;
  /** Custom styles */
  style?: React.CSSProperties;
}

const variantToColors: Record<
  NotificationVariant,
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

const priorityToColors: Record<
  NotificationPriority,
  {
    color: string;
    bg: string;
  }
> = {
  low: { color: "var(--color-neutral-400)", bg: "var(--color-neutral-800)" },
  medium: {
    color: "var(--color-semantic-info)",
    bg: "rgba(59, 130, 246, 0.1)",
  },
  high: {
    color: "var(--color-semantic-warning)",
    bg: "rgba(245, 158, 11, 0.1)",
  },
  urgent: {
    color: "var(--color-semantic-error)",
    bg: "rgba(239, 68, 68, 0.1)",
  },
};

const sizeToStyles: Record<
  "sm" | "md" | "lg",
  {
    width: string;
    maxHeight: string;
    padding: string;
    fontSize: string;
    iconSize: number;
  }
> = {
  sm: {
    width: "320px",
    maxHeight: "400px",
    padding: "8px",
    fontSize: "13px",
    iconSize: 16,
  },
  md: {
    width: "400px",
    maxHeight: "500px",
    padding: "12px",
    fontSize: "14px",
    iconSize: 20,
  },
  lg: {
    width: "480px",
    maxHeight: "600px",
    padding: "16px",
    fontSize: "15px",
    iconSize: 24,
  },
};

const positionToStyles: Record<string, React.CSSProperties> = {
  "top-right": { top: "16px", right: "16px" },
  "top-left": { top: "16px", left: "16px" },
  "bottom-right": { bottom: "16px", right: "16px" },
  "bottom-left": { bottom: "16px", left: "16px" },
};

export const NotificationCenter: React.FC<NotificationCenterProps> = ({
  notifications = [],
  open = false,
  onToggle,
  onNotificationClick,
  onMarkAsRead,
  onMarkAllAsRead,
  onArchive,
  onArchiveAll,
  onDelete,
  onClearAll,
  statusFilter = "all",
  categoryFilter = "all",
  priorityFilter = "all",
  maxNotifications = 50,
  showUnreadCount = true,
  groupByDate = true,
  showTimestamps = true,
  showCategories = false,
  showPriorities = false,
  emptyMessage = "No notifications",
  emptyIcon,
  position = "top-right",
  size = "md",
  dropdown = true,
  trigger,
  className = "",
  style = {},
}) => {
  const [internalOpen, setInternalOpen] = useState(open);
  const [filters, setFilters] = useState({
    status: statusFilter,
    category: categoryFilter,
    priority: priorityFilter,
  });

  const isOpen = open ?? internalOpen;
  const sizeStyles = sizeToStyles[size];
  const positionStyles = positionToStyles[position];

  // Filter notifications
  const filteredNotifications = useMemo(() => {
    let filtered = notifications;

    if (filters.status !== "all") {
      filtered = filtered.filter((n) => n.status === filters.status);
    }
    if (filters.category !== "all") {
      filtered = filtered.filter((n) => n.category === filters.category);
    }
    if (filters.priority !== "all") {
      filtered = filtered.filter((n) => n.priority === filters.priority);
    }

    // Sort by timestamp (newest first)
    filtered = filtered.sort(
      (a, b) => b.timestamp.getTime() - a.timestamp.getTime(),
    );

    // Limit number of notifications
    return filtered.slice(0, maxNotifications);
  }, [notifications, filters, maxNotifications]);

  // Group notifications by date
  const groupedNotifications = useMemo(() => {
    if (!groupByDate) return { All: filteredNotifications };

    const groups: Record<string, Notification[]> = {};
    const now = new Date();
    const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const yesterday = new Date(today);
    yesterday.setDate(yesterday.getDate() - 1);

    filteredNotifications.forEach((notification) => {
      const notificationDate = new Date(notification.timestamp);
      const notificationDay = new Date(
        notificationDate.getFullYear(),
        notificationDate.getMonth(),
        notificationDate.getDate(),
      );

      let groupKey: string;
      if (notificationDay.getTime() === today.getTime()) {
        groupKey = "Today";
      } else if (notificationDay.getTime() === yesterday.getTime()) {
        groupKey = "Yesterday";
      } else if (
        notificationDate.getTime() >
        today.getTime() - 7 * 24 * 60 * 60 * 1000
      ) {
        groupKey = "This Week";
      } else {
        groupKey = notificationDate.toLocaleDateString();
      }

      if (!groups[groupKey]) {
        groups[groupKey] = [];
      }
      groups[groupKey].push(notification);
    });

    return groups;
  }, [filteredNotifications, groupByDate]);

  // Calculate unread count
  const unreadCount = useMemo(() => {
    return notifications.filter((n) => n.status === "unread").length;
  }, [notifications]);

  // Handle toggle
  const handleToggle = useCallback(() => {
    const newOpen = !isOpen;
    setInternalOpen(newOpen);
    onToggle?.(newOpen);
  }, [isOpen, onToggle]);

  // Handle notification click
  const handleNotificationClick = useCallback(
    (notification: Notification) => {
      if (notification.status === "unread") {
        onMarkAsRead?.(notification.id);
      }
      onNotificationClick?.(notification);
    },
    [onMarkAsRead, onNotificationClick],
  );

  // Handle action click
  const handleActionClick = useCallback(
    (action: NotificationAction, notification: Notification) => {
      if (!action.disabled) {
        action.onClick(notification);
      }
    },
    [],
  );

  // Get default icon based on variant
  const getDefaultIcon = (variant: NotificationVariant = "info") => {
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

  // Format timestamp
  const formatTimestamp = (timestamp: Date) => {
    const now = new Date();
    const diff = now.getTime() - timestamp.getTime();
    const minutes = Math.floor(diff / 60000);
    const hours = Math.floor(diff / 3600000);
    const days = Math.floor(diff / 86400000);

    if (minutes < 1) return "Just now";
    if (minutes < 60) return `${minutes}m ago`;
    if (hours < 24) return `${hours}h ago`;
    if (days < 7) return `${days}d ago`;
    return timestamp.toLocaleDateString();
  };

  // Render notification item
  const renderNotification = (notification: Notification) => {
    const palette = variantToColors[notification.variant || "info"];
    const priorityColors = priorityToColors[notification.priority || "medium"];
    const isUnread = notification.status === "unread";

    return (
      <div
        key={notification.id}
        onClick={() => handleNotificationClick(notification)}
        style={{
          display: "flex",
          alignItems: "flex-start",
          gap: "12px",
          padding: "12px",
          borderRadius: "var(--radius-md)",
          border: `1px solid ${palette.border}`,
          background: isUnread
            ? palette.bg
            : "var(--color-background-secondary)",
          color: palette.text,
          cursor: "pointer",
          transition: "all 0.2s ease",
          position: "relative",
          marginBottom: "8px",
        }}
        onMouseEnter={(e) => {
          e.currentTarget.style.background = isUnread
            ? palette.bg
            : "var(--color-neutral-800)";
        }}
        onMouseLeave={(e) => {
          e.currentTarget.style.background = isUnread
            ? palette.bg
            : "var(--color-background-secondary)";
        }}
      >
        {/* Icon */}
        <div
          style={{
            lineHeight: 0,
            color: palette.icon,
            flexShrink: 0,
            marginTop: "2px",
          }}
        >
          {notification.icon || getDefaultIcon(notification.variant)}
        </div>

        {/* Content */}
        <div style={{ flex: 1, minWidth: 0 }}>
          <div
            style={{
              display: "flex",
              alignItems: "flex-start",
              justifyContent: "space-between",
              marginBottom: "4px",
            }}
          >
            <div
              style={{
                color: palette.title,
                fontWeight: isUnread ? 600 : 500,
                fontSize: sizeStyles.fontSize,
                lineHeight: 1.4,
              }}
            >
              {notification.title}
            </div>
            {showTimestamps && (
              <div
                style={{
                  fontSize: "12px",
                  color: "var(--color-neutral-400)",
                  marginLeft: "8px",
                  flexShrink: 0,
                }}
              >
                {formatTimestamp(notification.timestamp)}
              </div>
            )}
          </div>

          {notification.description && (
            <div
              style={{
                fontSize: sizeStyles.fontSize,
                lineHeight: 1.4,
                color: palette.text,
                marginBottom: "8px",
              }}
            >
              {notification.description}
            </div>
          )}

          {/* Metadata */}
          <div
            style={{
              display: "flex",
              gap: "8px",
              alignItems: "center",
              flexWrap: "wrap",
            }}
          >
            {showCategories && notification.category && (
              <Badge variant="neutral" size="sm">
                {notification.category}
              </Badge>
            )}
            {showPriorities && notification.priority && (
              <Badge
                variant="neutral"
                size="sm"
                style={{
                  color: priorityColors.color,
                  background: priorityColors.bg,
                }}
              >
                {notification.priority}
              </Badge>
            )}
          </div>

          {/* Actions */}
          {notification.actions && notification.actions.length > 0 && (
            <div
              style={{
                display: "flex",
                gap: "8px",
                marginTop: "8px",
                flexWrap: "wrap",
              }}
            >
              {notification.actions.map((action, index) => (
                <Button
                  key={index}
                  variant={action.variant || "secondary"}
                  onClick={(e) => {
                    e.stopPropagation();
                    handleActionClick(action, notification);
                  }}
                  disabled={action.disabled}
                  style={{
                    fontSize: "12px",
                    padding: "4px 8px",
                    minHeight: "auto",
                  }}
                >
                  {action.label}
                </Button>
              ))}
            </div>
          )}
        </div>

        {/* Actions menu */}
        <div
          style={{
            display: "flex",
            gap: "4px",
            flexShrink: 0,
            opacity: 0,
            transition: "opacity 0.2s ease",
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.opacity = "1";
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.opacity = "0";
          }}
        >
          {notification.status === "unread" && onMarkAsRead && (
            <button
              onClick={(e) => {
                e.stopPropagation();
                onMarkAsRead(notification.id);
              }}
              style={{
                background: "transparent",
                border: "none",
                color: "var(--color-neutral-400)",
                cursor: "pointer",
                padding: "4px",
                borderRadius: "4px",
                lineHeight: 0,
              }}
              title="Mark as read"
            >
              <Icon name="check" size="sm" />
            </button>
          )}
          {onArchive && (
            <button
              onClick={(e) => {
                e.stopPropagation();
                onArchive(notification.id);
              }}
              style={{
                background: "transparent",
                border: "none",
                color: "var(--color-neutral-400)",
                cursor: "pointer",
                padding: "4px",
                borderRadius: "4px",
                lineHeight: 0,
              }}
              title="Archive"
            >
              <Icon name="archive" size="sm" />
            </button>
          )}
          {onDelete && (
            <button
              onClick={(e) => {
                e.stopPropagation();
                onDelete(notification.id);
              }}
              style={{
                background: "transparent",
                border: "none",
                color: "var(--color-neutral-400)",
                cursor: "pointer",
                padding: "4px",
                borderRadius: "4px",
                lineHeight: 0,
              }}
              title="Delete"
            >
              <Icon name="trash" size="sm" />
            </button>
          )}
        </div>
      </div>
    );
  };

  // Render empty state
  const renderEmptyState = () => (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        padding: "40px 20px",
        textAlign: "center",
        color: "var(--color-neutral-400)",
      }}
    >
      {emptyIcon || (
        <Icon
          name="bell"
          size="2xl"
          style={{ marginBottom: "16px", opacity: 0.5 }}
        />
      )}
      <div style={{ fontSize: "16px", fontWeight: 500, marginBottom: "8px" }}>
        {emptyMessage}
      </div>
      <div style={{ fontSize: "14px", opacity: 0.7 }}>
        {filters.status !== "all" ||
        filters.category !== "all" ||
        filters.priority !== "all"
          ? "Try adjusting your filters"
          : "You're all caught up!"}
      </div>
    </div>
  );

  // Render notification center
  const renderNotificationCenter = () => (
    <div
      style={{
        position: "fixed",
        ...positionStyles,
        width: sizeStyles.width,
        maxHeight: sizeStyles.maxHeight,
        background: "var(--color-background-primary)",
        border: "1px solid var(--color-neutral-700)",
        borderRadius: "var(--radius-lg)",
        boxShadow: "0 20px 40px rgba(0, 0, 0, 0.4)",
        zIndex: 1000,
        display: "flex",
        flexDirection: "column",
        ...style,
      }}
      className={className}
    >
      {/* Header */}
      <div
        style={{
          padding: sizeStyles.padding,
          borderBottom: "1px solid var(--color-neutral-700)",
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
          <h3
            style={{
              margin: 0,
              fontSize: "16px",
              fontWeight: 600,
              color: "#FFFFFF",
            }}
          >
            Notifications
          </h3>
          {showUnreadCount && unreadCount > 0 && (
            <div
              style={{
                background: "var(--color-brand-600)",
                color: "#FFFFFF",
                borderRadius: "10px",
                minWidth: "20px",
                height: "20px",
                fontSize: "12px",
                fontWeight: "600",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                padding: "0 6px",
                lineHeight: 1,
              }}
            >
              {unreadCount > 99 ? "99+" : unreadCount}
            </div>
          )}
        </div>
        <div style={{ display: "flex", gap: "4px" }}>
          {onMarkAllAsRead && unreadCount > 0 && (
            <button
              onClick={onMarkAllAsRead}
              style={{
                background: "transparent",
                border: "none",
                color: "var(--color-neutral-400)",
                cursor: "pointer",
                padding: "4px",
                borderRadius: "4px",
                fontSize: "12px",
              }}
              title="Mark all as read"
            >
              Mark all read
            </button>
          )}
          {onClearAll && notifications.length > 0 && (
            <button
              onClick={onClearAll}
              style={{
                background: "transparent",
                border: "none",
                color: "var(--color-neutral-400)",
                cursor: "pointer",
                padding: "4px",
                borderRadius: "4px",
                fontSize: "12px",
              }}
              title="Clear all"
            >
              Clear all
            </button>
          )}
        </div>
      </div>

      {/* Filters */}
      <div
        style={{
          padding: sizeStyles.padding,
          borderBottom: "1px solid var(--color-neutral-700)",
          display: "flex",
          gap: "8px",
          flexWrap: "wrap",
        }}
      >
        <select
          value={filters.status}
          onChange={(e) =>
            setFilters((prev) => ({ ...prev, status: e.target.value as any }))
          }
          style={{
            background: "var(--color-background-secondary)",
            border: "1px solid var(--color-neutral-600)",
            borderRadius: "4px",
            color: "var(--color-neutral-300)",
            padding: "4px 8px",
            fontSize: "12px",
          }}
        >
          <option value="all">All</option>
          <option value="unread">Unread</option>
          <option value="read">Read</option>
          <option value="archived">Archived</option>
        </select>
        <select
          value={filters.category}
          onChange={(e) =>
            setFilters((prev) => ({ ...prev, category: e.target.value as any }))
          }
          style={{
            background: "var(--color-background-secondary)",
            border: "1px solid var(--color-neutral-600)",
            borderRadius: "4px",
            color: "var(--color-neutral-300)",
            padding: "4px 8px",
            fontSize: "12px",
          }}
        >
          <option value="all">All Categories</option>
          <option value="system">System</option>
          <option value="user">User</option>
          <option value="security">Security</option>
          <option value="feature">Feature</option>
          <option value="marketing">Marketing</option>
          <option value="maintenance">Maintenance</option>
        </select>
        <select
          value={filters.priority}
          onChange={(e) =>
            setFilters((prev) => ({ ...prev, priority: e.target.value as any }))
          }
          style={{
            background: "var(--color-background-secondary)",
            border: "1px solid var(--color-neutral-600)",
            borderRadius: "4px",
            color: "var(--color-neutral-300)",
            padding: "4px 8px",
            fontSize: "12px",
          }}
        >
          <option value="all">All Priorities</option>
          <option value="low">Low</option>
          <option value="medium">Medium</option>
          <option value="high">High</option>
          <option value="urgent">Urgent</option>
        </select>
      </div>

      {/* Notifications */}
      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: sizeStyles.padding,
        }}
      >
        {filteredNotifications.length === 0
          ? renderEmptyState()
          : Object.entries(groupedNotifications).map(
              ([groupName, groupNotifications]) => (
                <div key={groupName}>
                  {groupByDate && groupName !== "All" && (
                    <div
                      style={{
                        fontSize: "12px",
                        fontWeight: 600,
                        color: "var(--color-neutral-400)",
                        textTransform: "uppercase",
                        letterSpacing: "0.05em",
                        marginBottom: "12px",
                        marginTop:
                          groupName !== Object.keys(groupedNotifications)[0]
                            ? "24px"
                            : "0",
                      }}
                    >
                      {groupName}
                    </div>
                  )}
                  {groupNotifications.map(renderNotification)}
                </div>
              ),
            )}
      </div>
    </div>
  );

  if (dropdown) {
    return (
      <div style={{ position: "relative" }}>
        {trigger ? (
          <div onClick={handleToggle}>{trigger}</div>
        ) : (
          <button
            onClick={handleToggle}
            style={{
              background: "transparent",
              border: "none",
              color: "var(--color-neutral-300)",
              cursor: "pointer",
              padding: "8px",
              borderRadius: "4px",
              position: "relative",
            }}
          >
            <Icon name="bell" size="md" />
            {showUnreadCount && unreadCount > 0 && (
              <div
                style={{
                  position: "absolute",
                  top: "-2px",
                  right: "-2px",
                  background: "var(--color-semantic-error)",
                  color: "#FFFFFF",
                  borderRadius: "50%",
                  minWidth: "18px",
                  height: "18px",
                  fontSize: "10px",
                  fontWeight: "600",
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  padding: "0 4px",
                  lineHeight: 1,
                  border: "2px solid var(--color-background-primary)",
                  boxSizing: "border-box",
                }}
              >
                {unreadCount > 99 ? "99+" : unreadCount}
              </div>
            )}
          </button>
        )}
        {isOpen && renderNotificationCenter()}
      </div>
    );
  }

  return renderNotificationCenter();
};

// Hook for managing notifications
export const useNotifications = () => {
  const [notifications, setNotifications] = useState<Notification[]>([]);

  const addNotification = useCallback(
    (notification: Omit<Notification, "id" | "timestamp">) => {
      const newNotification: Notification = {
        id: Math.random().toString(36).slice(2),
        timestamp: new Date(),
        status: "unread",
        priority: "medium",
        category: "system",
        variant: "info",
        ...notification,
      };
      setNotifications((prev) => [newNotification, ...prev]);
      return newNotification.id;
    },
    [],
  );

  const markAsRead = useCallback((id: string) => {
    setNotifications((prev) =>
      prev.map((n) =>
        n.id === id
          ? { ...n, status: "read" as NotificationStatus, readAt: new Date() }
          : n,
      ),
    );
  }, []);

  const markAllAsRead = useCallback(() => {
    setNotifications((prev) =>
      prev.map((n) =>
        n.status === "unread"
          ? { ...n, status: "read" as NotificationStatus, readAt: new Date() }
          : n,
      ),
    );
  }, []);

  const archive = useCallback((id: string) => {
    setNotifications((prev) =>
      prev.map((n) =>
        n.id === id
          ? {
              ...n,
              status: "archived" as NotificationStatus,
              archivedAt: new Date(),
            }
          : n,
      ),
    );
  }, []);

  const archiveAll = useCallback(() => {
    setNotifications((prev) =>
      prev.map((n) =>
        n.status !== "archived"
          ? {
              ...n,
              status: "archived" as NotificationStatus,
              archivedAt: new Date(),
            }
          : n,
      ),
    );
  }, []);

  const deleteNotification = useCallback((id: string) => {
    setNotifications((prev) => prev.filter((n) => n.id !== id));
  }, []);

  const clearAll = useCallback(() => {
    setNotifications([]);
  }, []);

  return {
    notifications,
    addNotification,
    markAsRead,
    markAllAsRead,
    archive,
    archiveAll,
    deleteNotification,
    clearAll,
  };
};

export default NotificationCenter;
