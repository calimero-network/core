import React, { forwardRef, useState } from "react";
import { tokens } from "@calimero-network/mero-tokens";

export interface NumberInputProps {
  value?: number;
  defaultValue?: number;
  min?: number;
  max?: number;
  step?: number;
  precision?: number;
  disabled?: boolean;
  readOnly?: boolean;
  required?: boolean;
  placeholder?: string;
  size?: "sm" | "md" | "lg";
  variant?: "default" | "filled" | "outlined";
  error?: boolean;
  helperText?: string;
  label?: string;
  showButtons?: boolean;
  className?: string;
  style?: React.CSSProperties;
  onChange?: (value: number | undefined) => void;
  onFocus?: (event: React.FocusEvent<HTMLInputElement>) => void;
  onBlur?: (event: React.FocusEvent<HTMLInputElement>) => void;
}

export const NumberInput = forwardRef<HTMLInputElement, NumberInputProps>(
  (
    {
      value,
      defaultValue,
      min,
      max,
      step = 1,
      precision,
      disabled = false,
      readOnly = false,
      required = false,
      placeholder,
      size = "md",
      variant = "default",
      error = false,
      helperText,
      label,
      showButtons = true,
      className = "",
      style = {},
      onChange,
      onFocus,
      onBlur,
    },
    ref,
  ) => {
    const [internalValue, setInternalValue] = useState<number | undefined>(
      defaultValue,
    );
    const currentValue = value !== undefined ? value : internalValue;

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
        backgroundColor: "transparent",
        border: `1px solid ${error ? tokens.color.semantic.error.value : tokens.color.neutral[600].value}`,
      },
      filled: {
        backgroundColor: "transparent",
        border: `1px solid ${error ? tokens.color.semantic.error.value : "transparent"}`,
      },
      outlined: {
        backgroundColor: "transparent",
        border: `2px solid ${error ? tokens.color.semantic.error.value : tokens.color.neutral[600].value}`,
      },
    };

    const baseStyle = {
      width: "100%",
      fontFamily: "var(--font-body)", // Use design system font
      color: "#FFFFFF", // White text for better contrast with dark background
      borderRadius: tokens.radius.md.value,
      outline: "none",
      transition: "all 0.2s ease",
      fontWeight: "400",
      lineHeight: "1.5",
      ...sizeStyles[size],
      ...variantStyles[variant],
      ...style,
    };

    const containerStyle = {
      position: "relative" as const,
      display: "flex",
      alignItems: "center",
      width: "100%",
    };

    const inputStyle = {
      ...baseStyle,
      paddingRight: showButtons
        ? "40px"
        : sizeStyles[size].padding.split(" ")[1],
      textAlign: "right" as const,
    };

    const buttonStyle = {
      position: "absolute" as const,
      right: "1px",
      top: "1px",
      bottom: "1px",
      width: "32px",
      backgroundColor: "transparent",
      border: "none",
      color: "#FFFFFF", // White text for better contrast
      cursor: "pointer",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      fontSize: "12px",
      transition: "background-color 0.2s ease",
    };

    const incrementButtonStyle = {
      ...buttonStyle,
      borderTopRightRadius: tokens.radius.sm.value,
      borderBottomRightRadius: "0",
    };

    const decrementButtonStyle = {
      ...buttonStyle,
      top: "50%",
      height: "50%",
      borderBottomRightRadius: tokens.radius.sm.value,
      borderTopRightRadius: "0",
    };

    const labelStyle = {
      display: "block",
      fontSize: "14px",
      fontWeight: 500,
      color: "#FFFFFF", // White text for better contrast
      marginBottom: "4px",
      fontFamily: "var(--font-body)",
      lineHeight: "1.4",
    };

    const helperTextStyle = {
      fontSize: "12px",
      color: error
        ? tokens.color.semantic.error.value
        : tokens.color.neutral[300].value,
      marginTop: "4px",
      fontFamily: "var(--font-body)",
      lineHeight: "1.4",
    };

    const formatValue = (val: number | undefined): string => {
      if (val === undefined || val === null) return "";
      if (precision !== undefined) {
        return val.toFixed(precision);
      }
      return val.toString();
    };

    const parseValue = (str: string): number | undefined => {
      const parsed = parseFloat(str);
      if (isNaN(parsed)) return undefined;
      if (precision !== undefined) {
        return parseFloat(parsed.toFixed(precision));
      }
      return parsed;
    };

    const handleChange = (event: React.ChangeEvent<HTMLInputElement>) => {
      const newValue = parseValue(event.target.value);
      setInternalValue(newValue);
      onChange?.(newValue);
    };

    const handleIncrement = () => {
      if (disabled || readOnly) return;
      const newValue = currentValue !== undefined ? currentValue + step : step;
      const clampedValue =
        max !== undefined ? Math.min(newValue, max) : newValue;
      const finalValue =
        min !== undefined ? Math.max(clampedValue, min) : clampedValue;
      setInternalValue(finalValue);
      onChange?.(finalValue);
    };

    const handleDecrement = () => {
      if (disabled || readOnly) return;
      const newValue = currentValue !== undefined ? currentValue - step : -step;
      const clampedValue =
        min !== undefined ? Math.max(newValue, min) : newValue;
      const finalValue =
        max !== undefined ? Math.min(clampedValue, max) : clampedValue;
      setInternalValue(finalValue);
      onChange?.(finalValue);
    };

    const handleKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
      if (event.key === "ArrowUp") {
        event.preventDefault();
        handleIncrement();
      } else if (event.key === "ArrowDown") {
        event.preventDefault();
        handleDecrement();
      }
    };

    return (
      <div
        className={className}
        style={{ display: "flex", flexDirection: "column", width: "100%" }}
      >
        {label && (
          <label style={labelStyle}>
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
        <div style={containerStyle}>
          <input
            ref={ref}
            type="number"
            value={formatValue(currentValue)}
            min={min}
            max={max}
            step={step}
            disabled={disabled}
            readOnly={readOnly}
            required={required}
            placeholder={placeholder}
            style={inputStyle}
            onChange={handleChange}
            onFocus={onFocus}
            onBlur={onBlur}
            onKeyDown={handleKeyDown}
          />
          {showButtons && (
            <>
              <button
                type="button"
                style={incrementButtonStyle}
                onClick={handleIncrement}
                disabled={
                  disabled ||
                  readOnly ||
                  (max !== undefined &&
                    currentValue !== undefined &&
                    currentValue >= max)
                }
                onMouseEnter={(e) => {
                  if (!disabled && !readOnly) {
                    e.currentTarget.style.backgroundColor =
                      tokens.color.background.brand.value;
                  }
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.backgroundColor =
                    tokens.color.background.tertiary.value;
                }}
              >
                +
              </button>
              <button
                type="button"
                style={decrementButtonStyle}
                onClick={handleDecrement}
                disabled={
                  disabled ||
                  readOnly ||
                  (min !== undefined &&
                    currentValue !== undefined &&
                    currentValue <= min)
                }
                onMouseEnter={(e) => {
                  if (!disabled && !readOnly) {
                    e.currentTarget.style.backgroundColor =
                      tokens.color.background.brand.value;
                  }
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.backgroundColor =
                    tokens.color.background.tertiary.value;
                }}
              >
                −
              </button>
            </>
          )}
        </div>
        {helperText && <div style={helperTextStyle}>{helperText}</div>}
      </div>
    );
  },
);
