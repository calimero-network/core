import React, { useState, useCallback, useRef, useEffect } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Icon } from "./Icon";
import { Button } from "./Button";

// Navigation item types
export interface SidebarItem {
  id: string;
  label: string;
  icon?: string;
  href?: string;
  onClick?: () => void;
  disabled?: boolean;
  badge?: string | number;
  children?: SidebarItem[];
  external?: boolean;
}

export interface SidebarGroup {
  id: string;
  label?: string;
  items: SidebarItem[];
  collapsible?: boolean;
  defaultCollapsed?: boolean;
}

// Main Sidebar props
export interface SidebarProps {
  /** Navigation groups */
  groups: SidebarGroup[];
  /** Currently active item ID */
  activeItemId?: string;
  /** Callback when item is clicked */
  onItemClick?: (item: SidebarItem) => void;
  /** Sidebar variant */
  variant?: "default" | "minimal" | "compact";
  /** Sidebar size */
  size?: "sm" | "md" | "lg";
  /** Whether sidebar is collapsed */
  collapsed?: boolean;
  /** Whether sidebar can be toggled */
  collapsible?: boolean;
  /** Callback when collapse state changes */
  onCollapseChange?: (collapsed: boolean) => void;
  /** Sidebar position */
  position?: "left" | "right";
  /** Whether sidebar is fixed */
  fixed?: boolean;
  /** Custom header content */
  header?: React.ReactNode;
  /** Custom footer content */
  footer?: React.ReactNode;
  /** Custom className */
  className?: string;
  /** Custom styles */
  style?: React.CSSProperties;
  /** Whether to show group labels when collapsed */
  showGroupLabelsWhenCollapsed?: boolean;
  /** Whether to show tooltips when collapsed */
  showTooltipsWhenCollapsed?: boolean;
}

// Sidebar item component
interface SidebarItemComponentProps {
  item: SidebarItem;
  isActive: boolean;
  isCollapsed: boolean;
  showTooltip: boolean;
  onClick: (item: SidebarItem) => void;
  onToggleChildren?: (itemId: string) => void;
  hasChildren: boolean;
  isExpanded: boolean;
}

const SidebarItemComponent: React.FC<SidebarItemComponentProps> = ({
  item,
  isActive,
  isCollapsed,
  showTooltip,
  onClick,
  onToggleChildren,
  hasChildren,
  isExpanded,
}) => {
  const [isHovered, setIsHovered] = useState(false);

  const handleClick = () => {
    if (!item.disabled) {
      if (hasChildren) {
        onToggleChildren?.(item.id);
      } else {
        onClick(item);
      }
    }
  };

  const itemStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    padding: "8px 12px",
    margin: "2px 8px",
    borderRadius: "6px",
    cursor: item.disabled ? "not-allowed" : "pointer",
    color: item.disabled ? "#6B7280" : isActive ? "#FFFFFF" : "#E5E7EB",
    backgroundColor: isActive ? "#374151" : "transparent",
    fontSize: "14px",
    fontWeight: isActive ? "500" : "400",
    textDecoration: "none",
    transition: "all 0.15s ease",
    position: "relative",
    opacity: item.disabled ? 0.6 : 1,
  };

  if (isHovered && !item.disabled && !isActive) {
    itemStyle.backgroundColor = "#374151";
  }

  const iconStyle: React.CSSProperties = {
    marginRight: isCollapsed ? 0 : "8px",
    display: "flex",
    alignItems: "center",
    minWidth: "20px",
    justifyContent: "center",
  };

  const labelStyle: React.CSSProperties = {
    flex: 1,
    whiteSpace: "nowrap",
    overflow: "hidden",
    textOverflow: "ellipsis",
  };

  const badgeStyle: React.CSSProperties = {
    marginLeft: "8px",
    padding: "2px 6px",
    borderRadius: "10px",
    fontSize: "11px",
    fontWeight: "600",
    backgroundColor: tokens.color.brand[600].value,
    color: "#FFFFFF",
    minWidth: "18px",
    textAlign: "center",
  };

  const chevronStyle: React.CSSProperties = {
    marginLeft: "8px",
    display: "flex",
    alignItems: "center",
    transition: "transform 0.2s ease",
    transform: isExpanded ? "rotate(90deg)" : "rotate(0deg)",
  };

  return (
    <div
      style={itemStyle}
      onClick={handleClick}
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
      title={showTooltip ? item.label : undefined}
    >
      {item.icon && (
        <div style={iconStyle}>
          <Icon name={item.icon} size="sm" />
        </div>
      )}

      {!isCollapsed && <div style={labelStyle}>{item.label}</div>}

      {!isCollapsed && item.badge && <div style={badgeStyle}>{item.badge}</div>}

      {!isCollapsed && hasChildren && (
        <div style={chevronStyle}>
          <Icon name="chevron-right" size="xs" />
        </div>
      )}
    </div>
  );
};

// Sidebar group component
interface SidebarGroupComponentProps {
  group: SidebarGroup;
  activeItemId?: string;
  isCollapsed: boolean;
  showTooltip: boolean;
  onItemClick: (item: SidebarItem) => void;
  onToggleChildren: (itemId: string) => void;
  expandedItems: Set<string>;
}

const SidebarGroupComponent: React.FC<SidebarGroupComponentProps> = ({
  group,
  activeItemId,
  isCollapsed,
  showTooltip,
  onItemClick,
  onToggleChildren,
  expandedItems,
}) => {
  const [isGroupCollapsed, setIsGroupCollapsed] = useState(
    group.defaultCollapsed || false,
  );

  const handleGroupToggle = () => {
    if (group.collapsible) {
      setIsGroupCollapsed(!isGroupCollapsed);
    }
  };

  const groupHeaderStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    padding: "12px 16px 8px 16px",
    fontSize: "12px",
    fontWeight: "600",
    color: "#9CA3AF",
    textTransform: "uppercase",
    letterSpacing: "0.05em",
    cursor: group.collapsible ? "pointer" : "default",
    userSelect: "none",
  };

  const groupToggleStyle: React.CSSProperties = {
    marginLeft: "auto",
    display: "flex",
    alignItems: "center",
    transition: "transform 0.2s ease",
    transform: isGroupCollapsed ? "rotate(-90deg)" : "rotate(0deg)",
  };

  const renderItem = (item: SidebarItem, level: number = 0) => {
    const isActive = activeItemId === item.id;
    const hasChildren = Boolean(item.children && item.children.length > 0);
    const isExpanded = expandedItems.has(item.id);

    return (
      <div key={item.id}>
        <SidebarItemComponent
          item={item}
          isActive={isActive}
          isCollapsed={isCollapsed}
          showTooltip={showTooltip}
          onClick={onItemClick}
          onToggleChildren={onToggleChildren}
          hasChildren={hasChildren}
          isExpanded={isExpanded}
        />

        {hasChildren && isExpanded && !isCollapsed && (
          <div style={{ marginLeft: `${level * 16 + 20}px` }}>
            {item.children?.map((child) => renderItem(child, level + 1))}
          </div>
        )}
      </div>
    );
  };

  return (
    <div>
      {group.label && !isCollapsed && (
        <div style={groupHeaderStyle} onClick={handleGroupToggle}>
          {group.label}
          {group.collapsible && (
            <div style={groupToggleStyle}>
              <Icon name="chevron-down" size="xs" />
            </div>
          )}
        </div>
      )}

      {(!group.collapsible || !isGroupCollapsed) && (
        <div>{group.items.map((item) => renderItem(item))}</div>
      )}
    </div>
  );
};

// Main Sidebar component
export const Sidebar: React.FC<SidebarProps> = ({
  groups,
  activeItemId,
  onItemClick,
  variant = "default",
  size = "md",
  collapsed = false,
  collapsible = true,
  onCollapseChange,
  position = "left",
  fixed = false,
  header,
  footer,
  className = "",
  style,
  showGroupLabelsWhenCollapsed = false,
  showTooltipsWhenCollapsed = true,
}) => {
  const [internalCollapsed, setInternalCollapsed] = useState(
    collapsed ?? false,
  );
  const [expandedItems, setExpandedItems] = useState<Set<string>>(new Set());
  const sidebarRef = useRef<HTMLDivElement>(null);

  const isCollapsed = collapsed ?? internalCollapsed;
  const showTooltip = isCollapsed && showTooltipsWhenCollapsed;

  const handleToggleCollapse = useCallback(() => {
    if (collapsible) {
      const newCollapsed = !isCollapsed;
      setInternalCollapsed(newCollapsed);
      onCollapseChange?.(newCollapsed);
    }
  }, [collapsible, isCollapsed, onCollapseChange]);

  const handleItemClick = useCallback(
    (item: SidebarItem) => {
      onItemClick?.(item);
    },
    [onItemClick],
  );

  const handleToggleChildren = useCallback((itemId: string) => {
    setExpandedItems((prev) => {
      const newSet = new Set(prev);
      if (newSet.has(itemId)) {
        newSet.delete(itemId);
      } else {
        newSet.add(itemId);
      }
      return newSet;
    });
  }, []);

  // Size configurations
  const sizeConfig = {
    sm: {
      width: isCollapsed ? "48px" : "200px",
      padding: "8px",
      itemPadding: "6px 8px",
      fontSize: "13px",
    },
    md: {
      width: isCollapsed ? "56px" : "240px",
      padding: "12px",
      itemPadding: "8px 12px",
      fontSize: "14px",
    },
    lg: {
      width: isCollapsed ? "64px" : "280px",
      padding: "16px",
      itemPadding: "10px 16px",
      fontSize: "15px",
    },
  };

  const currentSize = sizeConfig[size];

  // Variant configurations
  const variantConfig = {
    default: {
      backgroundColor: "#1A1A1A",
      borderRight: "1px solid #404040",
    },
    minimal: {
      backgroundColor: "transparent",
      borderRight: "none",
    },
    compact: {
      backgroundColor: "#1A1A1A",
      borderRight: "1px solid #404040",
    },
  };

  const currentVariant = variantConfig[variant];

  const sidebarStyle: React.CSSProperties = {
    display: "flex",
    flexDirection: "column",
    width: currentSize.width,
    height: "100vh",
    backgroundColor: currentVariant.backgroundColor,
    borderRight: currentVariant.borderRight,
    transition: "width 0.3s ease",
    position: fixed ? "fixed" : "relative",
    top: fixed ? 0 : "auto",
    left: fixed && position === "left" ? 0 : "auto",
    right: fixed && position === "right" ? 0 : "auto",
    zIndex: fixed ? 1000 : "auto",
    overflow: "hidden",
    ...style,
  };

  const contentStyle: React.CSSProperties = {
    flex: 1,
    overflowY: "auto",
    overflowX: "hidden",
    padding: currentSize.padding,
  };

  const headerStyle: React.CSSProperties = {
    padding: currentSize.padding,
    borderBottom: "1px solid #404040",
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
  };

  const footerStyle: React.CSSProperties = {
    padding: currentSize.padding,
    borderTop: "1px solid #404040",
  };

  const toggleButtonStyle: React.CSSProperties = {
    position: "absolute",
    top: "12px",
    right: isCollapsed ? "8px" : "12px",
    zIndex: 1001,
    padding: "4px",
    minWidth: "auto",
    width: "24px",
    height: "24px",
  };

  return (
    <div ref={sidebarRef} className={className} style={sidebarStyle}>
      {/* Toggle Button */}
      {collapsible && (
        <Button
          onClick={handleToggleCollapse}
          variant="secondary"
          style={toggleButtonStyle}
        >
          <Icon
            name={isCollapsed ? "chevron-right" : "chevron-left"}
            size="xs"
          />
        </Button>
      )}

      {/* Header */}
      {header && <div style={headerStyle}>{header}</div>}

      {/* Navigation Content */}
      <div style={contentStyle}>
        {groups.map((group) => (
          <SidebarGroupComponent
            key={group.id}
            group={group}
            activeItemId={activeItemId}
            isCollapsed={isCollapsed}
            showTooltip={showTooltip}
            onItemClick={handleItemClick}
            onToggleChildren={handleToggleChildren}
            expandedItems={expandedItems}
          />
        ))}
      </div>

      {/* Footer */}
      {footer && <div style={footerStyle}>{footer}</div>}
    </div>
  );
};

// Convenience components for common patterns
export const SidebarHeader: React.FC<{
  children: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
}> = ({ children, className = "", style }) => (
  <div className={className} style={style}>
    {children}
  </div>
);

export const SidebarFooter: React.FC<{
  children: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
}> = ({ children, className = "", style }) => (
  <div className={className} style={style}>
    {children}
  </div>
);

export const SidebarBrand: React.FC<{
  children: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
}> = ({ children, className = "", style }) => (
  <div
    className={className}
    style={{
      fontSize: "18px",
      fontWeight: "600",
      color: "#FFFFFF",
      ...style,
    }}
  >
    {children}
  </div>
);

export const SidebarUser: React.FC<{
  name: string;
  email?: string;
  avatar?: string;
  className?: string;
  style?: React.CSSProperties;
}> = ({ name, email, avatar, className = "", style }) => (
  <div
    className={className}
    style={{
      display: "flex",
      alignItems: "center",
      gap: "8px",
      padding: "8px",
      borderRadius: "6px",
      backgroundColor: "#374151",
      ...style,
    }}
  >
    {avatar ? (
      <img
        src={avatar}
        alt={name}
        style={{
          width: "32px",
          height: "32px",
          borderRadius: "50%",
          objectFit: "cover",
        }}
      />
    ) : (
      <div
        style={{
          width: "32px",
          height: "32px",
          borderRadius: "50%",
          backgroundColor: tokens.color.brand[600].value,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          color: "#FFFFFF",
          fontSize: "14px",
          fontWeight: "600",
        }}
      >
        {name.charAt(0).toUpperCase()}
      </div>
    )}
    <div style={{ flex: 1, minWidth: 0 }}>
      <div
        style={{
          fontSize: "14px",
          fontWeight: "500",
          color: "#FFFFFF",
          whiteSpace: "nowrap",
          overflow: "hidden",
          textOverflow: "ellipsis",
        }}
      >
        {name}
      </div>
      {email && (
        <div
          style={{
            fontSize: "12px",
            color: "#9CA3AF",
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {email}
        </div>
      )}
    </div>
  </div>
);
