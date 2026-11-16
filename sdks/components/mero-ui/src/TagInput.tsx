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

export interface Tag {
  id: string;
  label: string;
  value: any;
  color?: string;
  removable?: boolean;
}

export interface TagInputProps {
  value?: Tag[];
  onChange?: (tags: Tag[]) => void;
  onAdd?: (tag: Tag) => void;
  onRemove?: (tag: Tag) => void;
  suggestions?: string[];
  placeholder?: string;
  disabled?: boolean;
  maxTags?: number;
  allowDuplicates?: boolean;
  validateTag?: (tag: string) => boolean | string;
  className?: string;
  style?: React.CSSProperties;
  error?: boolean;
  errorMessage?: string;
  label?: string;
  required?: boolean;
  size?: "small" | "medium" | "large";
  variant?: "default" | "outlined" | "filled";
  showSuggestions?: boolean;
  maxSuggestions?: number;
  createTagOnEnter?: boolean;
  createTagOnComma?: boolean;
  createTagOnSpace?: boolean;
  separator?: string;
  caseSensitive?: boolean;
}

export function TagInput({
  value = [],
  onChange,
  onAdd,
  onRemove,
  suggestions = [],
  placeholder = "Add tags...",
  disabled = false,
  maxTags,
  allowDuplicates = false,
  validateTag,
  className = "",
  style = {},
  error = false,
  errorMessage,
  label,
  required = false,
  size = "medium",
  variant = "default",
  showSuggestions = true,
  maxSuggestions = 10,
  createTagOnEnter = true,
  createTagOnComma = true,
  createTagOnSpace = false,
  separator = ",",
  caseSensitive = false,
}: TagInputProps) {
  const [inputValue, setInputValue] = useState("");
  const [isOpen, setIsOpen] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [validationError, setValidationError] = useState<string | null>(null);

  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const suggestionRefs = useRef<(HTMLDivElement | null)[]>([]);

  // Filter suggestions based on input
  const filteredSuggestions = useMemo(() => {
    if (!showSuggestions || !inputValue || inputValue.length === 0) {
      return [];
    }

    const filtered = suggestions.filter((suggestion) => {
      const searchTerm = caseSensitive ? inputValue : inputValue.toLowerCase();
      const suggestionText = caseSensitive
        ? suggestion
        : suggestion.toLowerCase();
      return suggestionText.includes(searchTerm);
    });

    return filtered.slice(0, maxSuggestions);
  }, [suggestions, inputValue, maxSuggestions, showSuggestions, caseSensitive]);

  // Create tag from input value
  const createTag = useCallback(
    (tagText: string): Tag | null => {
      const trimmedText = tagText.trim();
      if (!trimmedText) return null;

      // Check for duplicates
      if (
        !allowDuplicates &&
        value.some((tag) =>
          caseSensitive
            ? tag.label === trimmedText
            : tag.label.toLowerCase() === trimmedText.toLowerCase(),
        )
      ) {
        setValidationError("Tag already exists");
        return null;
      }

      // Validate tag
      if (validateTag) {
        const validation = validateTag(trimmedText);
        if (validation !== true) {
          setValidationError(
            typeof validation === "string" ? validation : "Invalid tag",
          );
          return null;
        }
      }

      // Check max tags
      if (maxTags && value.length >= maxTags) {
        setValidationError(`Maximum ${maxTags} tags allowed`);
        return null;
      }

      setValidationError(null);

      return {
        id: `${trimmedText}-${Date.now()}`,
        label: trimmedText,
        value: trimmedText,
      };
    },
    [value, allowDuplicates, caseSensitive, validateTag, maxTags],
  );

  // Add tag
  const addTag = useCallback(
    (tag: Tag) => {
      const newTags = [...value, tag];
      onChange?.(newTags);
      onAdd?.(tag);
      setInputValue("");
      setIsOpen(false);
      setSelectedIndex(0);
    },
    [value, onChange, onAdd],
  );

  // Remove tag
  const removeTag = useCallback(
    (tagToRemove: Tag) => {
      const newTags = value.filter((tag) => tag.id !== tagToRemove.id);
      onChange?.(newTags);
      onRemove?.(tagToRemove);
    },
    [value, onChange, onRemove],
  );

  // Handle input change
  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newValue = e.target.value;
    setInputValue(newValue);
    setValidationError(null);

    if (newValue.length > 0) {
      setIsOpen(true);
    } else {
      setIsOpen(false);
    }
  };

  // Handle input key down
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (disabled) return;

    switch (e.key) {
      case "Enter":
        e.preventDefault();
        if (isOpen && filteredSuggestions[selectedIndex]) {
          const suggestion = filteredSuggestions[selectedIndex];
          const tag = createTag(suggestion);
          if (tag) addTag(tag);
        } else if (createTagOnEnter && inputValue.trim()) {
          const tag = createTag(inputValue);
          if (tag) addTag(tag);
        }
        break;

      case "Backspace":
        if (!inputValue && value.length > 0) {
          e.preventDefault();
          const lastTag = value[value.length - 1];
          if (lastTag.removable !== false) {
            removeTag(lastTag);
          }
        }
        break;

      case "ArrowDown":
        e.preventDefault();
        if (isOpen) {
          setSelectedIndex((prev) =>
            prev < filteredSuggestions.length - 1 ? prev + 1 : 0,
          );
        }
        break;

      case "ArrowUp":
        e.preventDefault();
        if (isOpen) {
          setSelectedIndex((prev) =>
            prev > 0 ? prev - 1 : filteredSuggestions.length - 1,
          );
        }
        break;

      case "Escape":
        e.preventDefault();
        setIsOpen(false);
        setSelectedIndex(0);
        break;

      case ",":
        if (createTagOnComma) {
          e.preventDefault();
          const tag = createTag(inputValue);
          if (tag) addTag(tag);
        }
        break;

      case " ":
        if (createTagOnSpace) {
          e.preventDefault();
          const tag = createTag(inputValue);
          if (tag) addTag(tag);
        }
        break;
    }
  };

  // Handle suggestion selection
  const handleSuggestionSelect = (suggestion: string) => {
    const tag = createTag(suggestion);
    if (tag) addTag(tag);
  };

  // Handle input blur
  const handleInputBlur = () => {
    setTimeout(() => {
      setIsOpen(false);
      setSelectedIndex(0);
    }, 150);
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

  // Scroll selected suggestion into view
  useEffect(() => {
    if (suggestionRefs.current[selectedIndex]) {
      suggestionRefs.current[selectedIndex]?.scrollIntoView({
        block: "nearest",
        behavior: "smooth",
      });
    }
  }, [selectedIndex]);

  const sizeStyles = {
    small: {
      padding: "6px 8px",
      fontSize: "12px",
      minHeight: "32px",
      tagPadding: "2px 6px",
      tagFontSize: "11px",
    },
    medium: {
      padding: "8px 12px",
      fontSize: "14px",
      minHeight: "40px",
      tagPadding: "4px 8px",
      tagFontSize: "12px",
    },
    large: {
      padding: "12px 16px",
      fontSize: "16px",
      minHeight: "48px",
      tagPadding: "6px 10px",
      tagFontSize: "14px",
    },
  };

  const variantStyles = {
    default: {
      background: "transparent",
      border: `1px solid ${tokens.color.neutral[600].value}`,
    },
    outlined: {
      background: "transparent",
      border: `2px solid ${tokens.color.neutral[600].value}`,
    },
    filled: {
      background: "var(--color-background-secondary)",
      border: "none",
    },
  };

  const currentSizeStyle = sizeStyles[size];
  const currentVariantStyle = variantStyles[variant];

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

      <div
        style={{
          ...currentVariantStyle,
          ...currentSizeStyle,
          borderRadius: "6px",
          display: "flex",
          flexWrap: "wrap",
          alignItems: "center",
          gap: "6px",
          minHeight: currentSizeStyle.minHeight,
          cursor: disabled ? "not-allowed" : "text",
          opacity: disabled ? 0.6 : 1,
          transition: "all 0.2s ease",
        }}
        onClick={() => !disabled && inputRef.current?.focus()}
      >
        {/* Tags */}
        {value.map((tag) => (
          <div
            key={tag.id}
            style={{
              display: "flex",
              alignItems: "center",
              gap: "4px",
              background: tag.color || tokens.color.brand[600].value,
              color: "#000000",
              padding: currentSizeStyle.tagPadding,
              borderRadius: "4px",
              fontSize: currentSizeStyle.tagFontSize,
              fontWeight: "500",
            }}
          >
            <span>{tag.label}</span>
            {tag.removable !== false && (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  removeTag(tag);
                }}
                style={{
                  background: "none",
                  border: "none",
                  color: "#000000",
                  cursor: "pointer",
                  padding: "0",
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  fontSize: "12px",
                }}
              >
                <Icon name="x" size="sm" />
              </button>
            )}
          </div>
        ))}

        {/* Input */}
        <input
          ref={inputRef}
          type="text"
          value={inputValue}
          onChange={handleInputChange}
          onKeyDown={handleKeyDown}
          onBlur={handleInputBlur}
          placeholder={value.length === 0 ? placeholder : ""}
          disabled={disabled}
          style={{
            background: "none",
            border: "none",
            outline: "none",
            color: "#FFFFFF",
            fontSize: currentSizeStyle.fontSize,
            padding: "0",
            flex: 1,
            minWidth: "120px",
          }}
        />
      </div>

      {/* Validation Error */}
      {(error && errorMessage) || validationError ? (
        <div
          style={{
            fontSize: "12px",
            color: tokens.color.semantic.error.value,
            marginTop: "4px",
          }}
        >
          {validationError || errorMessage}
        </div>
      ) : null}

      {/* Suggestions Dropdown */}
      {isOpen && showSuggestions && filteredSuggestions.length > 0 && (
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
            maxHeight: "200px",
            overflow: "auto",
          }}
        >
          {filteredSuggestions.map((suggestion: string, index: number) => (
            <div
              key={suggestion}
              ref={(el: HTMLDivElement | null) => {
                suggestionRefs.current[index] = el;
              }}
              onClick={() => handleSuggestionSelect(suggestion)}
              style={{
                padding: "12px 16px",
                cursor: "pointer",
                background:
                  index === selectedIndex
                    ? tokens.color.brand[600].value + "20"
                    : "transparent",
                borderLeft:
                  index === selectedIndex
                    ? `3px solid ${tokens.color.brand[600].value}`
                    : "3px solid transparent",
                color: "#FFFFFF",
                fontSize: "14px",
                transition: "all 0.2s ease",
              }}
              onMouseEnter={(e) => {
                if (index !== selectedIndex) {
                  e.currentTarget.style.background =
                    "var(--color-background-secondary)";
                }
              }}
              onMouseLeave={(e) => {
                if (index !== selectedIndex) {
                  e.currentTarget.style.background = "transparent";
                }
              }}
            >
              {suggestion}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
