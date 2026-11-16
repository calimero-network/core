import React, { useState, useRef, useEffect } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Icon } from "./Icon";

export interface AccordionItemProps {
  title: React.ReactNode;
  children: React.ReactNode;
  disabled?: boolean;
  defaultOpen?: boolean;
  icon?: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
}

export interface AccordionProps {
  items: AccordionItemProps[];
  allowMultiple?: boolean;
  variant?: "default" | "bordered" | "filled";
  size?: "small" | "medium" | "large";
  className?: string;
  style?: React.CSSProperties;
}

export function AccordionItem({
  title,
  children,
  disabled = false,
  defaultOpen = false,
  icon,
  className = "",
  style = {},
}: AccordionItemProps) {
  const [isOpen, setIsOpen] = useState(defaultOpen);
  const [contentHeight, setContentHeight] = useState<number | undefined>(
    undefined,
  );
  const contentRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (contentRef.current) {
      setContentHeight(isOpen ? contentRef.current.scrollHeight : 0);
    }
  }, [isOpen, children]);

  const handleToggle = () => {
    if (!disabled) {
      setIsOpen(!isOpen);
    }
  };

  return (
    <div
      className={className}
      style={{
        border: `1px solid ${tokens.color.neutral[600].value}`,
        borderRadius: "8px",
        marginBottom: "8px",
        overflow: "hidden",
        ...style,
      }}
    >
      <button
        type="button"
        onClick={handleToggle}
        disabled={disabled}
        style={{
          width: "100%",
          padding: "16px 20px",
          background: "transparent",
          border: "none",
          cursor: disabled ? "not-allowed" : "pointer",
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          textAlign: "left",
          color: disabled ? tokens.color.neutral[400].value : "#FFFFFF",
          fontSize: "16px",
          fontWeight: 500,
          fontFamily: "var(--font-body)",
          transition: "all 0.2s ease",
          opacity: disabled ? 0.6 : 1,
        }}
        onMouseEnter={(e) => {
          if (!disabled) {
            e.currentTarget.style.background =
              "var(--color-background-secondary)";
          }
        }}
        onMouseLeave={(e) => {
          if (!disabled) {
            e.currentTarget.style.background = "transparent";
          }
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: "12px",
            flex: 1,
          }}
        >
          {icon && (
            <div
              style={{
                color: disabled
                  ? tokens.color.neutral[400].value
                  : tokens.color.brand[600].value,
                display: "flex",
                alignItems: "center",
              }}
            >
              {icon}
            </div>
          )}
          <div style={{ flex: 1 }}>{title}</div>
        </div>

        <Icon
          name="chevron-down"
          size="sm"
          style={{
            color: disabled
              ? tokens.color.neutral[400].value
              : tokens.color.neutral[400].value,
            transform: isOpen ? "rotate(180deg)" : "rotate(0deg)",
            transition: "transform 0.2s ease",
          }}
        />
      </button>

      <div
        style={{
          height: contentHeight,
          overflow: "hidden",
          transition: "height 0.3s ease",
        }}
      >
        <div
          ref={contentRef}
          style={{
            padding: "0 20px 20px 20px",
            color: tokens.color.neutral[300].value,
            lineHeight: 1.6,
          }}
        >
          {children}
        </div>
      </div>
    </div>
  );
}

export function Accordion({
  items,
  allowMultiple = false,
  variant = "default",
  size = "medium",
  className = "",
  style = {},
}: AccordionProps) {
  const [openItems, setOpenItems] = useState<Set<number>>(
    new Set(
      items
        .map((item, index) => (item.defaultOpen ? index : -1))
        .filter((i) => i !== -1),
    ),
  );

  const handleToggle = (index: number) => {
    setOpenItems((prev) => {
      const newOpenItems = new Set(prev);

      if (allowMultiple) {
        if (newOpenItems.has(index)) {
          newOpenItems.delete(index);
        } else {
          newOpenItems.add(index);
        }
      } else {
        newOpenItems.clear();
        if (!newOpenItems.has(index)) {
          newOpenItems.add(index);
        }
      }

      return newOpenItems;
    });
  };

  const variantStyles = {
    default: {
      background: "transparent",
      border: "none",
    },
    bordered: {
      background: "var(--color-background-primary)",
      border: `1px solid ${tokens.color.neutral[600].value}`,
      borderRadius: "8px",
      padding: "8px",
    },
    filled: {
      background: "var(--color-background-secondary)",
      border: "none",
      borderRadius: "8px",
      padding: "8px",
    },
  };

  const sizeStyles = {
    small: { padding: "12px 16px", fontSize: "14px" },
    medium: { padding: "16px 20px", fontSize: "16px" },
    large: { padding: "20px 24px", fontSize: "18px" },
  };

  const currentVariantStyle = variantStyles[variant];
  const currentSizeStyle = sizeStyles[size];

  return (
    <div
      className={className}
      style={{
        ...currentVariantStyle,
        ...style,
      }}
    >
      {items.map((item, index) => {
        const isOpen = openItems.has(index);

        return (
          <AccordionItem
            key={index}
            {...item}
            className=""
            style={{
              ...item.style,
              marginBottom: index < items.length - 1 ? "8px" : "0",
            }}
          />
        );
      })}
    </div>
  );
}

// Individual accordion item component for more control
export function AccordionItemControlled({
  title,
  children,
  isOpen,
  onToggle,
  disabled = false,
  icon,
  className = "",
  style = {},
}: AccordionItemProps & {
  isOpen: boolean;
  onToggle: () => void;
}) {
  const [contentHeight, setContentHeight] = useState<number | undefined>(
    undefined,
  );
  const contentRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (contentRef.current) {
      setContentHeight(isOpen ? contentRef.current.scrollHeight : 0);
    }
  }, [isOpen, children]);

  const handleToggle = () => {
    if (!disabled) {
      onToggle();
    }
  };

  return (
    <div
      className={className}
      style={{
        border: `1px solid ${tokens.color.neutral[600].value}`,
        borderRadius: "8px",
        marginBottom: "8px",
        overflow: "hidden",
        ...style,
      }}
    >
      <button
        type="button"
        onClick={handleToggle}
        disabled={disabled}
        style={{
          width: "100%",
          padding: "16px 20px",
          background: "transparent",
          border: "none",
          cursor: disabled ? "not-allowed" : "pointer",
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          textAlign: "left",
          color: disabled ? tokens.color.neutral[400].value : "#FFFFFF",
          fontSize: "16px",
          fontWeight: 500,
          fontFamily: "var(--font-body)",
          transition: "all 0.2s ease",
          opacity: disabled ? 0.6 : 1,
        }}
        onMouseEnter={(e) => {
          if (!disabled) {
            e.currentTarget.style.background =
              "var(--color-background-secondary)";
          }
        }}
        onMouseLeave={(e) => {
          if (!disabled) {
            e.currentTarget.style.background = "transparent";
          }
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: "12px",
            flex: 1,
          }}
        >
          {icon && (
            <div
              style={{
                color: disabled
                  ? tokens.color.neutral[400].value
                  : tokens.color.brand[600].value,
                display: "flex",
                alignItems: "center",
              }}
            >
              {icon}
            </div>
          )}
          <div style={{ flex: 1 }}>{title}</div>
        </div>

        <Icon
          name="chevron-down"
          size="sm"
          style={{
            color: disabled
              ? tokens.color.neutral[400].value
              : tokens.color.neutral[400].value,
            transform: isOpen ? "rotate(180deg)" : "rotate(0deg)",
            transition: "transform 0.2s ease",
          }}
        />
      </button>

      <div
        style={{
          height: contentHeight,
          overflow: "hidden",
          transition: "height 0.3s ease",
        }}
      >
        <div
          ref={contentRef}
          style={{
            padding: "0 20px 20px 20px",
            color: tokens.color.neutral[300].value,
            lineHeight: 1.6,
          }}
        >
          {children}
        </div>
      </div>
    </div>
  );
}
