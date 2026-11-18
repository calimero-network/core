import React from "react";

export type SelectOption = {
  value: string;
  label: string;
  disabled?: boolean;
};

export type SelectProps = {
  value?: string;
  defaultValue?: string;
  onChange?: (value: string) => void;
  options: SelectOption[];
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  style?: React.CSSProperties;
  name?: string;
  required?: boolean;
  error?: string;
  label?: string;
  description?: string;
};

export function Select({
  value,
  defaultValue,
  onChange,
  options,
  placeholder = "Select an option...",
  disabled = false,
  className = "",
  style,
  name,
  required = false,
  error,
  label,
  description,
}: SelectProps) {
  const [internalValue, setInternalValue] = React.useState(defaultValue || "");
  const [isOpen, setIsOpen] = React.useState(false);
  const [focusedIndex, setFocusedIndex] = React.useState(-1);
  const selectRef = React.useRef<HTMLDivElement>(null);
  const listRef = React.useRef<HTMLUListElement>(null);

  const isControlled = typeof value === "string";
  const currentValue = isControlled ? value : internalValue;
  const selectedOption = options.find(
    (option) => option.value === currentValue,
  );

  const handleChange = (newValue: string) => {
    if (!isControlled) {
      setInternalValue(newValue);
    }
    onChange?.(newValue);
    setIsOpen(false);
    setFocusedIndex(-1);
  };

  const handleKeyDown = (event: React.KeyboardEvent) => {
    if (!isOpen) {
      if (
        event.key === "Enter" ||
        event.key === " " ||
        event.key === "ArrowDown"
      ) {
        event.preventDefault();
        setIsOpen(true);
        setFocusedIndex(0);
      }
      return;
    }

    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        setFocusedIndex((prev) => Math.min(prev + 1, options.length - 1));
        break;
      case "ArrowUp":
        event.preventDefault();
        setFocusedIndex((prev) => Math.max(prev - 1, 0));
        break;
      case "Enter":
        event.preventDefault();
        if (focusedIndex >= 0 && focusedIndex < options.length) {
          const option = options[focusedIndex];
          if (!option.disabled) {
            handleChange(option.value);
          }
        }
        break;
      case "Escape":
        event.preventDefault();
        setIsOpen(false);
        setFocusedIndex(-1);
        break;
    }
  };

  React.useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        selectRef.current &&
        !selectRef.current.contains(event.target as Node)
      ) {
        setIsOpen(false);
        setFocusedIndex(-1);
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  React.useEffect(() => {
    if (isOpen && listRef.current && focusedIndex >= 0) {
      const focusedElement = listRef.current.children[
        focusedIndex
      ] as HTMLElement;
      if (focusedElement) {
        focusedElement.scrollIntoView({ block: "nearest" });
      }
    }
  }, [focusedIndex, isOpen]);

  const selectId = React.useId();

  return (
    <div className={className} style={style}>
      {label && (
        <label
          htmlFor={selectId}
          style={{
            display: "block",
            color: disabled ? "var(--color-neutral-600)" : "#FFFFFF",
            fontSize: "14px",
            fontWeight: 500,
            marginBottom: "4px",
            fontFamily: "var(--font-body)",
            lineHeight: "1.4",
          }}
        >
          {label}
          {required && (
            <span
              style={{
                color: "var(--color-semantic-error)",
                marginLeft: "2px",
              }}
            >
              *
            </span>
          )}
        </label>
      )}
      {description && (
        <div
          style={{
            color: "var(--color-neutral-400)",
            fontSize: "12px",
            marginBottom: "8px",
            fontFamily: "var(--font-body)",
            lineHeight: "1.4",
          }}
        >
          {description}
        </div>
      )}
      <div ref={selectRef} style={{ position: "relative" }}>
        <button
          type="button"
          id={selectId}
          name={name}
          onClick={() => !disabled && setIsOpen(!isOpen)}
          onKeyDown={handleKeyDown}
          disabled={disabled}
          style={{
            width: "100%",
            height: "40px",
            padding: "0 12px",
            border: `1px solid ${error ? "var(--color-semantic-error)" : "var(--color-neutral-600)"}`,
            borderRadius: "var(--radius-md)",
            backgroundColor: "transparent",
            color: selectedOption ? "#FFFFFF" : "var(--color-neutral-400)",
            fontSize: "14px",
            fontFamily: "var(--font-body)",
            fontWeight: "400",
            lineHeight: "1.5",
            textAlign: "left",
            cursor: disabled ? "not-allowed" : "pointer",
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            opacity: disabled ? 0.5 : 1,
          }}
        >
          <span>{selectedOption ? selectedOption.label : placeholder}</span>
          <svg
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            style={{
              transform: isOpen ? "rotate(180deg)" : "rotate(0deg)",
              transition: "transform 120ms ease",
            }}
          >
            <polyline points="6,9 12,15 18,9" />
          </svg>
        </button>
        {isOpen && (
          <ul
            ref={listRef}
            role="listbox"
            style={{
              position: "absolute",
              top: "100%",
              left: 0,
              right: 0,
              backgroundColor: "var(--color-background-primary)",
              border: "1px solid var(--color-neutral-600)",
              borderRadius: "var(--radius-md)",
              boxShadow: "0 8px 32px rgba(0, 0, 0, 0.3)",
              zIndex: 1000,
              maxHeight: "200px",
              overflowY: "auto",
              margin: 0,
              padding: "4px 0",
              listStyle: "none",
            }}
          >
            {options.map((option, index) => (
              <li
                key={option.value}
                role="option"
                aria-selected={option.value === currentValue}
                onClick={() => !option.disabled && handleChange(option.value)}
                style={{
                  padding: "8px 12px",
                  color: option.disabled
                    ? "var(--color-neutral-600)"
                    : "#FFFFFF",
                  backgroundColor: "transparent",
                  cursor: option.disabled ? "not-allowed" : "pointer",
                  fontSize: "14px",
                  fontFamily: "var(--font-body)",
                  fontWeight: "400",
                  lineHeight: "1.5",
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                }}
              >
                <span>{option.label}</span>
                {option.value === currentValue && (
                  <svg
                    width="16"
                    height="16"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="var(--color-brand-600)"
                    strokeWidth="2"
                  >
                    <polyline points="20,6 9,17 4,12" />
                  </svg>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
      {error && (
        <div
          style={{
            color: "var(--color-semantic-error)",
            fontSize: "12px",
            marginTop: "4px",
            fontFamily: "var(--font-body)",
            lineHeight: "1.4",
          }}
        >
          {error}
        </div>
      )}
    </div>
  );
}
