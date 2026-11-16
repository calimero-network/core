import React from "react";

export type NavbarProps = React.PropsWithChildren<{
  /** Optional className passthrough */
  className?: string;
  /** Custom inline styles */
  style?: React.CSSProperties;
  /** Navbar variant */
  variant?: "default" | "minimal" | "elevated";
  /** Navbar size */
  size?: "sm" | "md" | "lg";
  /** Whether the navbar is fixed */
  fixed?: boolean;
  /** Position when fixed */
  position?: "top" | "bottom";
}>;

export function Navbar({
  children,
  className = "",
  style,
  variant = "default",
  size = "md",
  fixed = false,
  position = "top",
}: NavbarProps) {
  const height = size === "sm" ? "48px" : size === "lg" ? "72px" : "60px";
  const padding =
    size === "sm" ? "0 16px" : size === "lg" ? "0 32px" : "0 24px";

  const baseStyles: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    width: "100%",
    height,
    padding,
    backgroundColor: "#1A1A1A",
    borderBottom: variant === "minimal" ? "none" : "1px solid #404040",
    boxShadow: variant === "elevated" ? "0 2px 8px rgba(0, 0, 0, 0.3)" : "none",
    position: fixed ? "fixed" : "relative",
    top: fixed && position === "top" ? 0 : undefined,
    bottom: fixed && position === "bottom" ? 0 : undefined,
    left: fixed ? 0 : undefined,
    right: fixed ? 0 : undefined,
    zIndex: fixed ? 1000 : "auto",
  };

  return (
    <nav
      className={className}
      style={{
        ...baseStyles,
        ...style,
      }}
    >
      {children}
    </nav>
  );
}

export type NavbarBrandProps = React.PropsWithChildren<{
  /** Brand logo/icon */
  logo?: React.ReactNode;
  /** Brand text */
  text?: string;
  /** Click handler for brand */
  onClick?: () => void;
  /** Optional className passthrough */
  className?: string;
  /** Custom inline styles */
  style?: React.CSSProperties;
}>;

export function NavbarBrand({
  children,
  logo,
  text,
  onClick,
  className = "",
  style,
}: NavbarBrandProps) {
  return (
    <div
      className={className}
      onClick={onClick}
      style={{
        display: "flex",
        alignItems: "center",
        gap: "12px",
        cursor: onClick ? "pointer" : "default",
        color: "white",
        fontSize: "18px",
        fontWeight: "600",
        textDecoration: "none",
        ...style,
      }}
    >
      {logo && (
        <div style={{ display: "flex", alignItems: "center" }}>{logo}</div>
      )}
      {text && <span>{text}</span>}
      {children}
    </div>
  );
}

export type NavbarItemProps = React.PropsWithChildren<{
  /** Whether the item is active */
  active?: boolean;
  /** Click handler */
  onClick?: () => void;
  /** Optional className passthrough */
  className?: string;
  /** Custom inline styles */
  style?: React.CSSProperties;
  /** Icon to display before the content */
  icon?: React.ReactNode;
  /** Whether the item is disabled */
  disabled?: boolean;
}>;

export function NavbarItem({
  children,
  active = false,
  onClick,
  className = "",
  style,
  icon,
  disabled = false,
}: NavbarItemProps) {
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
        gap: "8px",
        padding: "8px 16px",
        borderRadius: "6px",
        cursor: disabled ? "not-allowed" : "pointer",
        color: disabled ? "#6B7280" : active ? "#FFFFFF" : "#E5E7EB",
        backgroundColor: active ? "#374151" : "transparent",
        fontSize: "14px",
        fontWeight: active ? "500" : "400",
        textDecoration: "none",
        transition: "all 0.15s ease",
        ...style,
      }}
      onMouseEnter={(e) => {
        if (!disabled && !active) {
          e.currentTarget.style.backgroundColor = "#374151";
        }
      }}
      onMouseLeave={(e) => {
        if (!disabled && !active) {
          e.currentTarget.style.backgroundColor = "transparent";
        }
      }}
    >
      {icon && (
        <div style={{ display: "flex", alignItems: "center" }}>{icon}</div>
      )}
      {children}
    </div>
  );
}

export type NavbarMenuProps = React.PropsWithChildren<{
  /** Menu alignment */
  align?: "left" | "center" | "right";
  /** Optional className passthrough */
  className?: string;
  /** Custom inline styles */
  style?: React.CSSProperties;
}>;

export function NavbarMenu({
  children,
  align = "left",
  className = "",
  style,
}: NavbarMenuProps) {
  const justifyContent =
    align === "left"
      ? "flex-start"
      : align === "center"
        ? "center"
        : "flex-end";

  return (
    <div
      className={className}
      style={{
        display: "flex",
        alignItems: "center",
        gap: "8px",
        justifyContent,
        ...style,
      }}
    >
      {children}
    </div>
  );
}

export type NavbarToggleProps = {
  /** Whether the menu is open */
  isOpen?: boolean;
  /** Toggle handler */
  onToggle?: () => void;
  /** Optional className passthrough */
  className?: string;
  /** Custom inline styles */
  style?: React.CSSProperties;
};

export function NavbarToggle({
  isOpen = false,
  onToggle,
  className = "",
  style,
}: NavbarToggleProps) {
  return (
    <button
      className={className}
      onClick={onToggle}
      style={{
        display: "flex",
        flexDirection: "column",
        justifyContent: "center",
        alignItems: "center",
        width: "32px",
        height: "32px",
        background: "transparent",
        border: "none",
        cursor: "pointer",
        padding: "4px",
        ...style,
      }}
    >
      <div
        style={{
          width: "20px",
          height: "2px",
          backgroundColor: "white",
          margin: "2px 0",
          transition: "all 0.3s ease",
          transform: isOpen ? "rotate(45deg) translate(5px, 5px)" : "none",
        }}
      />
      <div
        style={{
          width: "20px",
          height: "2px",
          backgroundColor: "white",
          margin: "2px 0",
          transition: "all 0.3s ease",
          opacity: isOpen ? 0 : 1,
        }}
      />
      <div
        style={{
          width: "20px",
          height: "2px",
          backgroundColor: "white",
          margin: "2px 0",
          transition: "all 0.3s ease",
          transform: isOpen ? "rotate(-45deg) translate(7px, -6px)" : "none",
        }}
      />
    </button>
  );
}

export default Navbar;
