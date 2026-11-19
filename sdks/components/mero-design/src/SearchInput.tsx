import React, {
  useState,
  useRef,
  useEffect,
  useCallback,
  useMemo,
} from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Input } from "./Input";
import { Icon } from "./Icon";

export interface SearchSuggestion {
  id: string;
  text: string;
  description?: string;
  icon?: React.ReactNode;
  category?: string;
  value?: any;
}

export interface SearchInputProps {
  value?: string;
  onChange?: (value: string) => void;
  onSearch?: (query: string) => void;
  onSelect?: (suggestion: SearchSuggestion) => void;
  suggestions?: SearchSuggestion[];
  placeholder?: string;
  disabled?: boolean;
  loading?: boolean;
  clearable?: boolean;
  showSuggestions?: boolean;
  maxSuggestions?: number;
  minLength?: number;
  debounceMs?: number;
  className?: string;
  style?: React.CSSProperties;
  error?: boolean;
  errorMessage?: string;
  label?: string;
  required?: boolean;
  size?: "small" | "medium" | "large";
  showCategories?: boolean;
  emptyMessage?: string;
  hotkey?: string;
}

export function SearchInput({
  value = "",
  onChange,
  onSearch,
  onSelect,
  suggestions = [],
  placeholder = "Search...",
  disabled = false,
  loading = false,
  clearable = true,
  showSuggestions = true,
  maxSuggestions = 10,
  minLength = 1,
  debounceMs = 300,
  className = "",
  style = {},
  error = false,
  errorMessage,
  label,
  required = false,
  size = "medium",
  showCategories = true,
  emptyMessage = "No suggestions found",
  hotkey,
}: SearchInputProps) {
  const [inputValue, setInputValue] = useState(value);
  const [isOpen, setIsOpen] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [debouncedValue, setDebouncedValue] = useState(value);

  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const suggestionRefs = useRef<(HTMLDivElement | null)[]>([]);
  const debounceTimeoutRef = useRef<number | undefined>(undefined);

  // Debounce input value
  useEffect(() => {
    if (debounceTimeoutRef.current) {
      clearTimeout(debounceTimeoutRef.current);
    }

    debounceTimeoutRef.current = setTimeout(() => {
      setDebouncedValue(inputValue);
    }, debounceMs);

    return () => {
      if (debounceTimeoutRef.current) {
        clearTimeout(debounceTimeoutRef.current);
      }
    };
  }, [inputValue, debounceMs]);

  // Filter suggestions based on input
  const filteredSuggestions = useMemo(() => {
    if (!showSuggestions || !inputValue || inputValue.length < minLength) {
      return [];
    }

    const filtered = suggestions.filter(
      (suggestion) =>
        suggestion.text.toLowerCase().includes(inputValue.toLowerCase()) ||
        suggestion.description
          ?.toLowerCase()
          .includes(inputValue.toLowerCase()),
    );

    return filtered.slice(0, maxSuggestions);
  }, [suggestions, inputValue, minLength, maxSuggestions, showSuggestions]);

  // Group suggestions by category
  const groupedSuggestions = useMemo(() => {
    if (!showCategories) return { All: filteredSuggestions };

    return filteredSuggestions.reduce(
      (groups, suggestion) => {
        const category = suggestion.category || "Other";
        if (!groups[category]) groups[category] = [];
        groups[category].push(suggestion);
        return groups;
      },
      {} as Record<string, SearchSuggestion[]>,
    );
  }, [filteredSuggestions, showCategories]);

  // Handle input change
  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newValue = e.target.value;
    setInputValue(newValue);
    onChange?.(newValue);
    setSelectedIndex(0);

    if (newValue.length >= minLength) {
      setIsOpen(true);
    } else {
      setIsOpen(false);
    }
  };

  // Handle input focus
  const handleInputFocus = () => {
    if (inputValue.length >= minLength && filteredSuggestions.length > 0) {
      setIsOpen(true);
    }
  };

  // Handle input blur
  const handleInputBlur = (e: React.FocusEvent) => {
    // Delay closing to allow clicking on suggestions
    setTimeout(() => {
      if (!containerRef.current?.contains(document.activeElement)) {
        setIsOpen(false);
      }
    }, 150);
  };

  // Handle key navigation
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (!isOpen || filteredSuggestions.length === 0) return;

    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        setSelectedIndex((prev) =>
          prev < filteredSuggestions.length - 1 ? prev + 1 : 0,
        );
        break;
      case "ArrowUp":
        e.preventDefault();
        setSelectedIndex((prev) =>
          prev > 0 ? prev - 1 : filteredSuggestions.length - 1,
        );
        break;
      case "Enter":
        e.preventDefault();
        if (filteredSuggestions[selectedIndex]) {
          handleSuggestionSelect(filteredSuggestions[selectedIndex]);
        } else {
          onSearch?.(inputValue);
        }
        break;
      case "Escape":
        e.preventDefault();
        setIsOpen(false);
        break;
    }
  };

  // Handle suggestion selection
  const handleSuggestionSelect = (suggestion: SearchSuggestion) => {
    setInputValue(suggestion.text);
    onChange?.(suggestion.text);
    onSelect?.(suggestion);
    setIsOpen(false);
    setSelectedIndex(0);
  };

  // Handle clear
  const handleClear = () => {
    setInputValue("");
    onChange?.("");
    setIsOpen(false);
    setSelectedIndex(0);
    inputRef.current?.focus();
  };

  // Handle search
  const handleSearch = () => {
    onSearch?.(inputValue);
    setIsOpen(false);
  };

  // Close on outside click
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        containerRef.current &&
        !containerRef.current.contains(event.target as Node)
      ) {
        setIsOpen(false);
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  // Focus input when opened
  useEffect(() => {
    if (isOpen && inputRef.current) {
      inputRef.current.focus();
    }
  }, [isOpen]);

  // Scroll selected suggestion into view
  useEffect(() => {
    if (suggestionRefs.current[selectedIndex]) {
      suggestionRefs.current[selectedIndex]?.scrollIntoView({
        block: "nearest",
        behavior: "smooth",
      });
    }
  }, [selectedIndex]);

  // Handle hotkey
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (hotkey && e.key === hotkey && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        inputRef.current?.focus();
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [hotkey]);

  const sizeStyles = {
    small: { height: "32px", fontSize: "12px" },
    medium: { height: "40px", fontSize: "14px" },
    large: { height: "48px", fontSize: "16px" },
  };

  const currentSizeStyle = sizeStyles[size];

  return (
    <div
      ref={containerRef}
      className={className}
      style={{ position: "relative", ...style }}
    >
      {label && (
        <label
          style={{
            display: "block",
            fontSize: "14px",
            fontWeight: 500,
            color: "#FFFFFF",
            marginBottom: "8px",
            fontFamily: "var(--font-body)",
          }}
        >
          {label}
          {required && (
            <span
              style={{
                color: tokens.color.semantic.error.value,
                marginLeft: "4px",
              }}
            >
              *
            </span>
          )}
        </label>
      )}

      <div style={{ position: "relative" }}>
        <div
          onFocus={handleInputFocus}
          onBlur={handleInputBlur}
          onKeyDown={handleKeyDown}
        >
          <Input
            value={inputValue}
            onChange={handleInputChange}
            placeholder={placeholder}
            disabled={disabled}
            style={{
              ...currentSizeStyle,
              paddingRight: clearable ? "80px" : "40px",
            }}
          />
        </div>

        <div
          style={{
            position: "absolute",
            right: "8px",
            top: "50%",
            transform: "translateY(-50%)",
            display: "flex",
            alignItems: "center",
            gap: "4px",
          }}
        >
          {loading && (
            <div
              style={{
                width: "16px",
                height: "16px",
                border: `2px solid ${tokens.color.neutral[600].value}`,
                borderTop: `2px solid ${tokens.color.brand[600].value}`,
                borderRadius: "50%",
                animation: "spin 1s linear infinite",
              }}
            />
          )}

          {clearable && inputValue && !loading && (
            <button
              type="button"
              onClick={handleClear}
              style={{
                background: "none",
                border: "none",
                color: tokens.color.neutral[400].value,
                cursor: "pointer",
                padding: "4px",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
              }}
            >
              <Icon name="x" size="sm" />
            </button>
          )}

          <button
            type="button"
            onClick={handleSearch}
            style={{
              background: "none",
              border: "none",
              color: tokens.color.neutral[400].value,
              cursor: "pointer",
              padding: "4px",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
            }}
          >
            <Icon name="search" size="sm" />
          </button>
        </div>
      </div>

      {error && errorMessage && (
        <div
          style={{
            fontSize: "12px",
            color: tokens.color.semantic.error.value,
            marginTop: "4px",
          }}
        >
          {errorMessage}
        </div>
      )}

      {/* Suggestions Dropdown */}
      {isOpen && showSuggestions && (
        <div
          style={{
            position: "absolute",
            top: "100%",
            left: 0,
            right: 0,
            zIndex: 1000,
            background: "var(--color-background-primary)",
            border: `1px solid ${tokens.color.neutral[600].value}`,
            borderRadius: "8px",
            boxShadow: "0 4px 12px rgba(0, 0, 0, 0.3)",
            marginTop: "4px",
            maxHeight: "300px",
            overflow: "auto",
          }}
        >
          {Object.keys(groupedSuggestions).length === 0 ? (
            <div
              style={{
                padding: "16px",
                textAlign: "center",
                color: tokens.color.neutral[400].value,
              }}
            >
              {emptyMessage}
            </div>
          ) : (
            Object.entries(groupedSuggestions).map(
              ([category, categorySuggestions]) => (
                <div key={category}>
                  {showCategories && (
                    <div
                      style={{
                        padding: "8px 16px",
                        fontSize: "12px",
                        fontWeight: "600",
                        color: tokens.color.neutral[400].value,
                        background: "var(--color-background-secondary)",
                        textTransform: "uppercase",
                        letterSpacing: "0.5px",
                      }}
                    >
                      {category}
                    </div>
                  )}
                  {(categorySuggestions as SearchSuggestion[]).map(
                    (suggestion: SearchSuggestion, index: number) => {
                      const globalIndex = filteredSuggestions.findIndex(
                        (s: SearchSuggestion) => s.id === suggestion.id,
                      );
                      const isSelected = globalIndex === selectedIndex;

                      return (
                        <div
                          key={suggestion.id}
                          ref={(el: HTMLDivElement | null) => {
                            suggestionRefs.current[globalIndex] = el;
                          }}
                          onClick={() => handleSuggestionSelect(suggestion)}
                          style={{
                            padding: "12px 16px",
                            cursor: "pointer",
                            background: isSelected
                              ? tokens.color.brand[600].value + "20"
                              : "transparent",
                            borderLeft: isSelected
                              ? `3px solid ${tokens.color.brand[600].value}`
                              : "3px solid transparent",
                            display: "flex",
                            alignItems: "center",
                            gap: "12px",
                            transition: "all 0.2s ease",
                          }}
                          onMouseEnter={(e) => {
                            if (!isSelected) {
                              e.currentTarget.style.background =
                                "var(--color-background-secondary)";
                            }
                          }}
                          onMouseLeave={(e) => {
                            if (!isSelected) {
                              e.currentTarget.style.background = "transparent";
                            }
                          }}
                        >
                          {suggestion.icon && (
                            <div
                              style={{
                                color: isSelected
                                  ? tokens.color.brand[600].value
                                  : tokens.color.neutral[400].value,
                                display: "flex",
                                alignItems: "center",
                              }}
                            >
                              {suggestion.icon}
                            </div>
                          )}

                          <div style={{ flex: 1, minWidth: 0 }}>
                            <div
                              style={{
                                fontSize: "14px",
                                fontWeight: 500,
                                color: isSelected ? "#FFFFFF" : "#FFFFFF",
                                marginBottom: suggestion.description
                                  ? "2px"
                                  : "0",
                                overflow: "hidden",
                                textOverflow: "ellipsis",
                                whiteSpace: "nowrap",
                              }}
                            >
                              {suggestion.text}
                            </div>
                            {suggestion.description && (
                              <div
                                style={{
                                  fontSize: "12px",
                                  color: isSelected
                                    ? tokens.color.neutral[300].value
                                    : tokens.color.neutral[400].value,
                                  overflow: "hidden",
                                  textOverflow: "ellipsis",
                                  whiteSpace: "nowrap",
                                }}
                              >
                                {suggestion.description}
                              </div>
                            )}
                          </div>
                        </div>
                      );
                    },
                  )}
                </div>
              ),
            )
          )}
        </div>
      )}
    </div>
  );
}
