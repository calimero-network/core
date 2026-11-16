import React from "react";

export type MenuProps = React.PropsWithChildren<{
  /** Optional className passthrough */
  className?: string;
  /** Custom inline styles */
  style?: React.CSSProperties;
  /** Menu variant */
  variant?: "default" | "compact";
  /** Menu size */
  size?: "sm" | "md" | "lg";
}>;

export function Menu({
  children,
  className = "",
  style,
  variant = "default",
  size = "md",
}: MenuProps) {
  const padding = size === "sm" ? "4px" : size === "lg" ? "12px" : "8px";

  return (
    <div
      className={className}
      style={{
        background: "#1A1A1A",
        border: "1px solid #404040",
        borderRadius: "8px",
        padding,
        minWidth: size === "sm" ? "120px" : size === "lg" ? "200px" : "160px",
        boxShadow: "0 4px 12px rgba(0, 0, 0, 0.3)",
        ...style,
      }}
    >
      {children}
    </div>
  );
}

export type MenuItemProps = React.PropsWithChildren<{
  /** Whether the menu item is disabled */
  disabled?: boolean;
  /** Whether the menu item is selected */
  selected?: boolean;
  /** Click handler */
  onClick?: () => void;
  /** Optional className passthrough */
  className?: string;
  /** Custom inline styles */
  style?: React.CSSProperties;
  /** Icon to display before the content */
  icon?: React.ReactNode;
  /** Icon to display after the content */
  endIcon?: React.ReactNode;
}>;

export function MenuItem({
  children,
  disabled = false,
  selected = false,
  onClick,
  className = "",
  style,
  icon,
  endIcon,
}: MenuItemProps) {
  const handleClick = () => {
    if (!disabled && onClick) {
      onClick();
    }
  };

  return (
    <div
      className={className}
      onClick={handleClick}
      style={{
        display: "flex",
        alignItems: "center",
        padding: "8px 12px",
        borderRadius: "4px",
        cursor: disabled ? "not-allowed" : "pointer",
        color: disabled ? "#6B7280" : selected ? "#FFFFFF" : "#E5E7EB",
        backgroundColor: selected ? "#374151" : "transparent",
        fontSize: "14px",
        fontWeight: selected ? "500" : "400",
        transition: "all 0.15s ease",
        marginBottom: "2px",
        ...style,
      }}
      onMouseEnter={(e) => {
        if (!disabled && !selected) {
          e.currentTarget.style.backgroundColor = "#374151";
        }
      }}
      onMouseLeave={(e) => {
        if (!disabled && !selected) {
          e.currentTarget.style.backgroundColor = "transparent";
        }
      }}
    >
      {icon && (
        <div
          style={{ marginRight: "8px", display: "flex", alignItems: "center" }}
        >
          {icon}
        </div>
      )}
      <div style={{ flex: 1 }}>{children}</div>
      {endIcon && (
        <div
          style={{ marginLeft: "8px", display: "flex", alignItems: "center" }}
        >
          {endIcon}
        </div>
      )}
    </div>
  );
}

export type MenuGroupProps = React.PropsWithChildren<{
  /** Group label */
  label?: string;
  /** Optional className passthrough */
  className?: string;
  /** Custom inline styles */
  style?: React.CSSProperties;
}>;

export function MenuGroup({
  children,
  label,
  className = "",
  style,
}: MenuGroupProps) {
  return (
    <div className={className} style={style}>
      {label && (
        <div
          style={{
            padding: "8px 12px 4px 12px",
            fontSize: "12px",
            fontWeight: "600",
            color: "#9CA3AF",
            textTransform: "uppercase",
            letterSpacing: "0.05em",
          }}
        >
          {label}
        </div>
      )}
      <div>{children}</div>
    </div>
  );
}

export type MenuDividerProps = {
  /** Optional className passthrough */
  className?: string;
  /** Custom inline styles */
  style?: React.CSSProperties;
};

export function MenuDivider({ className = "", style }: MenuDividerProps) {
  return (
    <div
      className={className}
      style={{
        height: "1px",
        backgroundColor: "#404040",
        margin: "8px 12px",
        ...style,
      }}
    />
  );
}

export default Menu;
