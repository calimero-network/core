import React, { useState, useRef, useEffect, useCallback } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Icon } from "./Icon";

interface DropdownItem {
  value: string;
  label: string;
  disabled?: boolean;
  icon?: React.ReactNode;
  onClick?: () => void;
}

interface DropdownProps {
  items: DropdownItem[];
  value?: string;
  placeholder?: string;
  disabled?: boolean;
  size?: "sm" | "md" | "lg";
  variant?: "default" | "filled" | "outlined";
  onSelect?: (value: string, item: DropdownItem) => void;
  className?: string;
  style?: React.CSSProperties;
}

export const Dropdown: React.FC<DropdownProps> = ({
  items,
  value,
  placeholder = "Select an option",
  disabled = false,
  size = "md",
  variant = "default",
  onSelect,
  className = "",
  style = {},
}) => {
  const [isOpen, setIsOpen] = useState(false);
  const [selectedItem, setSelectedItem] = useState<DropdownItem | null>(
    items.find((item) => item.value === value) || null,
  );
  const dropdownRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  const sizeStyles = {
    sm: {
      padding: "6px 8px",
      fontSize: "14px",
      height: "32px",
    },
    md: {
      padding: "8px 12px",
      fontSize: "16px",
      height: "40px",
    },
    lg: {
      padding: "12px 16px",
      fontSize: "18px",
      height: "48px",
    },
  };

  const variantStyles = {
    default: {
      backgroundColor: tokens.color.background.primary.value,
      border: `1px solid ${tokens.color.neutral[600].value}`,
    },
    filled: {
      backgroundColor: tokens.color.background.tertiary.value,
      border: `1px solid ${disabled ? tokens.color.neutral[400].value : "transparent"}`,
    },
    outlined: {
      backgroundColor: "transparent",
      border: `2px solid ${tokens.color.neutral[600].value}`,
    },
  };

  const triggerStyle: React.CSSProperties = {
    width: "100%",
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    fontFamily: "system-ui, sans-serif",
    color: tokens.color.neutral[200].value,
    borderRadius: tokens.radius.md.value,
    outline: "none",
    cursor: disabled ? "not-allowed" : "pointer",
    transition: "all 0.2s ease",
    ...sizeStyles[size],
    ...variantStyles[variant],
    opacity: disabled ? 0.6 : 1,
  };

  const menuStyle: React.CSSProperties = {
    position: "absolute",
    top: "100%",
    left: 0,
    right: 0,
    zIndex: 1000,
    backgroundColor: tokens.color.background.primary.value,
    border: `1px solid ${tokens.color.neutral[600].value}`,
    borderRadius: tokens.radius.md.value,
    boxShadow:
      "0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -2px rgba(0, 0, 0, 0.05)",
    marginTop: "4px",
    maxHeight: "200px",
    overflowY: "auto",
    opacity: isOpen ? 1 : 0,
    visibility: isOpen ? "visible" : "hidden",
    transition: "opacity 0.2s ease, visibility 0.2s ease",
  };

  const itemStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    gap: tokens.space["2"].value,
    padding: "8px 12px",
    fontSize: "14px",
    color: tokens.color.neutral[200].value,
    cursor: "pointer",
    transition: "background-color 0.2s ease",
  };

  const handleToggle = () => {
    if (!disabled) {
      setIsOpen(!isOpen);
    }
  };

  const handleSelect = (item: DropdownItem) => {
    if (item.disabled) return;

    setSelectedItem(item);
    setIsOpen(false);
    onSelect?.(item.value, item);
    item.onClick?.();
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      setIsOpen(false);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      handleToggle();
    }
  };

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        dropdownRef.current &&
        !dropdownRef.current.contains(event.target as Node)
      ) {
        setIsOpen(false);
      }
    };

    if (isOpen) {
      document.addEventListener("mousedown", handleClickOutside);
      return () =>
        document.removeEventListener("mousedown", handleClickOutside);
    }
  }, [isOpen]);

  return (
    <div
      ref={dropdownRef}
      className={className}
      style={{ position: "relative", width: "100%", ...style }}
    >
      <button
        ref={triggerRef}
        type="button"
        style={triggerStyle}
        onClick={handleToggle}
        onKeyDown={handleKeyDown}
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={isOpen}
      >
        <span
          style={{
            display: "flex",
            alignItems: "center",
            gap: tokens.space["2"].value,
          }}
        >
          {selectedItem?.icon}
          {selectedItem ? selectedItem.label : placeholder}
        </span>
        <Icon
          name="chevron-down"
          size="sm"
          color="current"
          style={{
            transform: isOpen ? "rotate(180deg)" : "rotate(0deg)",
            transition: "transform 0.2s ease",
          }}
        />
      </button>

      {isOpen && (
        <div style={menuStyle} role="listbox">
          {items.map((item) => (
            <div
              key={item.value}
              style={{
                ...itemStyle,
                backgroundColor: item.disabled
                  ? tokens.color.background.tertiary.value
                  : selectedItem?.value === item.value
                    ? tokens.color.background.brand.value
                    : "transparent",
                color: item.disabled
                  ? tokens.color.neutral[300].value
                  : tokens.color.neutral[200].value,
                cursor: item.disabled ? "not-allowed" : "pointer",
                opacity: item.disabled ? 0.6 : 1,
              }}
              onClick={() => handleSelect(item)}
              onMouseEnter={(e) => {
                if (!item.disabled) {
                  e.currentTarget.style.backgroundColor =
                    tokens.color.background.brand.value;
                }
              }}
              onMouseLeave={(e) => {
                if (!item.disabled) {
                  e.currentTarget.style.backgroundColor =
                    selectedItem?.value === item.value
                      ? tokens.color.background.brand.value
                      : "transparent";
                }
              }}
            >
              {item.icon}
              {item.label}
            </div>
          ))}
        </div>
      )}
    </div>
  );
};
