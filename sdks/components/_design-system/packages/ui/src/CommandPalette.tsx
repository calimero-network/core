import React, { useState, useEffect, useRef, useMemo } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Input } from "./Input";
import { Icon } from "./Icon";

export interface CommandItem {
  id: string;
  title: string;
  description?: string;
  icon?: React.ReactNode;
  keywords?: string[];
  category?: string;
  action: () => void;
  disabled?: boolean;
}

export interface CommandPaletteProps {
  items: CommandItem[];
  placeholder?: string;
  emptyMessage?: string;
  maxHeight?: number;
  onOpen?: () => void;
  onClose?: () => void;
  onSelect?: (item: CommandItem) => void;
  className?: string;
  style?: React.CSSProperties;
  hotkey?: string;
  showCategories?: boolean;
  searchThreshold?: number;
}

export function CommandPalette({
  items,
  placeholder = "Type a command or search...",
  emptyMessage = "No commands found",
  maxHeight = 400,
  onOpen,
  onClose,
  onSelect,
  className = "",
  style = {},
  hotkey = "cmd+k",
  showCategories = true,
  searchThreshold = 0.3,
}: CommandPaletteProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [searchTerm, setSearchTerm] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [filteredItems, setFilteredItems] = useState<CommandItem[]>([]);
  const [groupedItems, setGroupedItems] = useState<
    Record<string, CommandItem[]>
  >({});

  const inputRef = useRef<HTMLInputElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<(HTMLDivElement | null)[]>([]);

  // Filter and group items based on search term
  const processItems = useMemo(() => {
    if (!searchTerm.trim()) {
      const grouped = items.reduce(
        (acc, item) => {
          const category = item.category || "Other";
          if (!acc[category]) acc[category] = [];
          acc[category].push(item);
          return acc;
        },
        {} as Record<string, CommandItem[]>,
      );
      return { filtered: items, grouped };
    }

    const filtered = items.filter((item) => {
      if (item.disabled) return false;

      const searchLower = searchTerm.toLowerCase();
      const titleMatch = item.title.toLowerCase().includes(searchLower);
      const descriptionMatch = item.description
        ?.toLowerCase()
        .includes(searchLower);
      const keywordMatch = item.keywords?.some((keyword) =>
        keyword.toLowerCase().includes(searchLower),
      );

      return titleMatch || descriptionMatch || keywordMatch;
    });

    const grouped = filtered.reduce(
      (acc, item) => {
        const category = item.category || "Other";
        if (!acc[category]) acc[category] = [];
        acc[category].push(item);
        return acc;
      },
      {} as Record<string, CommandItem[]>,
    );

    return { filtered, grouped };
  }, [items, searchTerm]);

  useEffect(() => {
    setFilteredItems(processItems.filtered);
    setGroupedItems(processItems.grouped);
    setSelectedIndex(0);
  }, [processItems]);

  // Handle keyboard shortcuts
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (hotkey && e.metaKey && e.key === "k") {
        e.preventDefault();
        setIsOpen(true);
      }

      if (e.key === "Escape" && isOpen) {
        setIsOpen(false);
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [hotkey, isOpen]);

  // Handle keyboard navigation
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (!isOpen) return;

    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        setSelectedIndex((prev) =>
          prev < filteredItems.length - 1 ? prev + 1 : 0,
        );
        break;
      case "ArrowUp":
        e.preventDefault();
        setSelectedIndex((prev) =>
          prev > 0 ? prev - 1 : filteredItems.length - 1,
        );
        break;
      case "Enter":
        e.preventDefault();
        if (
          filteredItems[selectedIndex] &&
          !filteredItems[selectedIndex].disabled
        ) {
          handleSelect(filteredItems[selectedIndex]);
        }
        break;
      case "Escape":
        e.preventDefault();
        setIsOpen(false);
        break;
    }
  };

  // Handle item selection
  const handleSelect = (item: CommandItem) => {
    if (item.disabled) return;

    item.action();
    onSelect?.(item);
    setIsOpen(false);
    setSearchTerm("");
  };

  // Handle open/close
  const handleOpen = () => {
    setIsOpen(true);
    onOpen?.();
  };

  const handleClose = () => {
    setIsOpen(false);
    setSearchTerm("");
    setSelectedIndex(0);
    onClose?.();
  };

  // Focus input when opened
  useEffect(() => {
    if (isOpen && inputRef.current) {
      inputRef.current.focus();
    }
  }, [isOpen]);

  // Scroll selected item into view
  useEffect(() => {
    if (itemRefs.current[selectedIndex]) {
      itemRefs.current[selectedIndex]?.scrollIntoView({
        block: "nearest",
        behavior: "smooth",
      });
    }
  }, [selectedIndex]);

  // Close on outside click
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        containerRef.current &&
        !containerRef.current.contains(event.target as Node)
      ) {
        handleClose();
      }
    };

    if (isOpen) {
      document.addEventListener("mousedown", handleClickOutside);
      return () =>
        document.removeEventListener("mousedown", handleClickOutside);
    }
  }, [isOpen]);

  if (!isOpen) {
    return (
      <button
        onClick={handleOpen}
        className={className}
        style={{
          background: "var(--color-background-secondary)",
          border: `1px solid ${tokens.color.neutral[600].value}`,
          borderRadius: "8px",
          padding: "8px 12px",
          color: tokens.color.neutral[400].value,
          fontSize: "14px",
          cursor: "pointer",
          display: "flex",
          alignItems: "center",
          gap: "8px",
          transition: "all 0.2s ease",
          ...style,
        }}
        onMouseEnter={(e) => {
          e.currentTarget.style.background = "var(--color-background-primary)";
          e.currentTarget.style.borderColor = tokens.color.neutral[400].value;
        }}
        onMouseLeave={(e) => {
          e.currentTarget.style.background =
            "var(--color-background-secondary)";
          e.currentTarget.style.borderColor = tokens.color.neutral[600].value;
        }}
      >
        <Icon name="search" size="sm" />
        <span>Search commands...</span>
        <kbd
          style={{
            background: tokens.color.neutral[700].value,
            padding: "2px 6px",
            borderRadius: "4px",
            fontSize: "12px",
            fontFamily: "monospace",
          }}
        >
          {hotkey.replace("cmd", "⌘")}
        </kbd>
      </button>
    );
  }

  return (
    <div
      ref={containerRef}
      className={className}
      style={{
        position: "fixed",
        top: "50%",
        left: "50%",
        transform: "translate(-50%, -50%)",
        width: "100%",
        maxWidth: "600px",
        background: "var(--color-background-primary)",
        border: `1px solid ${tokens.color.neutral[600].value}`,
        borderRadius: "12px",
        boxShadow:
          "0 20px 25px -5px rgba(0, 0, 0, 0.3), 0 10px 10px -5px rgba(0, 0, 0, 0.1)",
        zIndex: 1000,
        ...style,
      }}
    >
      {/* Search Input */}
      <div
        style={{
          padding: "16px",
          borderBottom: `1px solid ${tokens.color.neutral[600].value}`,
        }}
      >
        <div onKeyDown={handleKeyDown}>
          <Input
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
            placeholder={placeholder}
            style={{
              background: "transparent",
              border: "none",
              fontSize: "16px",
              padding: "0",
            }}
          />
        </div>
      </div>

      {/* Results */}
      <div style={{ maxHeight, overflow: "auto" }}>
        {filteredItems.length === 0 ? (
          <div
            style={{
              padding: "32px",
              textAlign: "center",
              color: tokens.color.neutral[400].value,
            }}
          >
            <Icon
              name="search"
              size="md"
              style={{ marginBottom: "12px", opacity: 0.5 }}
            />
            <div>{emptyMessage}</div>
          </div>
        ) : showCategories ? (
          // Grouped by categories
          Object.entries(groupedItems).map(([category, categoryItems]) => (
            <div key={category}>
              <div
                style={{
                  padding: "8px 16px",
                  fontSize: "12px",
                  fontWeight: 600,
                  color: tokens.color.neutral[400].value,
                  background: "var(--color-background-secondary)",
                  textTransform: "uppercase",
                  letterSpacing: "0.5px",
                }}
              >
                {category}
              </div>
              {categoryItems.map((item, index) => {
                const globalIndex = filteredItems.findIndex(
                  (filteredItem) => filteredItem.id === item.id,
                );
                const isSelected = globalIndex === selectedIndex;

                return (
                  <div
                    key={item.id}
                    ref={(el: HTMLDivElement | null) => {
                      itemRefs.current[globalIndex] = el;
                    }}
                    onClick={() => handleSelect(item)}
                    style={{
                      padding: "12px 16px",
                      cursor: item.disabled ? "not-allowed" : "pointer",
                      background: isSelected
                        ? tokens.color.brand[600].value + "20"
                        : "transparent",
                      borderLeft: isSelected
                        ? `3px solid ${tokens.color.brand[600].value}`
                        : "3px solid transparent",
                      display: "flex",
                      alignItems: "center",
                      gap: "12px",
                      opacity: item.disabled ? 0.5 : 1,
                      transition: "all 0.2s ease",
                    }}
                    onMouseEnter={(e) => {
                      if (!item.disabled && !isSelected) {
                        e.currentTarget.style.background =
                          "var(--color-background-secondary)";
                      }
                    }}
                    onMouseLeave={(e) => {
                      if (!item.disabled && !isSelected) {
                        e.currentTarget.style.background = "transparent";
                      }
                    }}
                  >
                    {item.icon && (
                      <div
                        style={{
                          color: isSelected
                            ? tokens.color.brand[600].value
                            : tokens.color.neutral[400].value,
                          display: "flex",
                          alignItems: "center",
                        }}
                      >
                        {item.icon}
                      </div>
                    )}

                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div
                        style={{
                          fontSize: "14px",
                          fontWeight: 500,
                          color: isSelected ? "#FFFFFF" : "#FFFFFF",
                          marginBottom: item.description ? "2px" : "0",
                        }}
                      >
                        {item.title}
                      </div>
                      {item.description && (
                        <div
                          style={{
                            fontSize: "12px",
                            color: isSelected
                              ? tokens.color.neutral[300].value
                              : tokens.color.neutral[400].value,
                          }}
                        >
                          {item.description}
                        </div>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          ))
        ) : (
          // Flat list
          filteredItems.map((item, index) => {
            const isSelected = index === selectedIndex;

            return (
              <div
                key={item.id}
                ref={(el: HTMLDivElement | null) => {
                  itemRefs.current[index] = el;
                }}
                onClick={() => handleSelect(item)}
                style={{
                  padding: "12px 16px",
                  cursor: item.disabled ? "not-allowed" : "pointer",
                  background: isSelected
                    ? tokens.color.brand[600].value + "20"
                    : "transparent",
                  borderLeft: isSelected
                    ? `3px solid ${tokens.color.brand[600].value}`
                    : "3px solid transparent",
                  display: "flex",
                  alignItems: "center",
                  gap: "12px",
                  opacity: item.disabled ? 0.5 : 1,
                  transition: "all 0.2s ease",
                }}
                onMouseEnter={(e) => {
                  if (!item.disabled && !isSelected) {
                    e.currentTarget.style.background =
                      "var(--color-background-secondary)";
                  }
                }}
                onMouseLeave={(e) => {
                  if (!item.disabled && !isSelected) {
                    e.currentTarget.style.background = "transparent";
                  }
                }}
              >
                {item.icon && (
                  <div
                    style={{
                      color: isSelected
                        ? tokens.color.brand[600].value
                        : tokens.color.neutral[400].value,
                      display: "flex",
                      alignItems: "center",
                    }}
                  >
                    {item.icon}
                  </div>
                )}

                <div style={{ flex: 1, minWidth: 0 }}>
                  <div
                    style={{
                      fontSize: "14px",
                      fontWeight: 500,
                      color: isSelected ? "#FFFFFF" : "#FFFFFF",
                      marginBottom: item.description ? "2px" : "0",
                    }}
                  >
                    {item.title}
                  </div>
                  {item.description && (
                    <div
                      style={{
                        fontSize: "12px",
                        color: isSelected
                          ? tokens.color.neutral[300].value
                          : tokens.color.neutral[400].value,
                      }}
                    >
                      {item.description}
                    </div>
                  )}
                </div>
              </div>
            );
          })
        )}
      </div>

      {/* Footer */}
      <div
        style={{
          padding: "12px 16px",
          borderTop: `1px solid ${tokens.color.neutral[600].value}`,
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          fontSize: "12px",
          color: tokens.color.neutral[400].value,
        }}
      >
        <div style={{ display: "flex", gap: "16px" }}>
          <span>↑↓ Navigate</span>
          <span>↵ Select</span>
          <span>⎋ Close</span>
        </div>
        <div>
          {filteredItems.length} command{filteredItems.length !== 1 ? "s" : ""}
        </div>
      </div>
    </div>
  );
}
